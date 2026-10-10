//! Zero-setup offline AI: bundled llama-server sidecar + managed GGUF model.
//!
//! Two pieces make offline AI work with no user configuration beyond one
//! click:
//!
//! 1. **Engine** — `llama-server.exe` ships as a Tauri sidecar
//!    (`bundle.externalBin: ["binaries/llama-server"]`), spawned from Rust via
//!    `tauri_plugin_shell` on 127.0.0.1. If it is not present (e.g. running in
//!    dev), we fall back to a system Ollama.
//! 2. **Model** — Qwen2.5-Coder-7B Q4_K_M downloaded once into the app data
//!    dir (never the install dir — updates replace that), verified by
//!    SHA-256, then served with `-m <path> --host 127.0.0.1 --port 8080
//!    -ngl 0 --no-webui`. Progress streams to the frontend as
//!    `llama-model-download` events; nothing is downloaded without the user
//!    pressing the button.
//!
//! Everything stays loopback-only. No telemetry, no keys, no DNS for
//! inference; the only outbound request is the one-time model file fetch.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;
use tauri::Emitter;

/// Known loopback OpenAI-compatible endpoints, probed in order.
pub const LOCAL_ENDPOINTS: [&str; 5] = [
    "http://localhost:11434/v1", // Ollama
    "http://localhost:8080/v1",  // llama.cpp llama-server (sidecar)
    "http://localhost:1234/v1",  // LM Studio
    "http://localhost:8000/v1",  // vLLM
    "http://localhost:1337/v1",  // llama-server alt
];

/// The one model the zero-setup flow installs. Apache-2.0; SHA-256 is pinned
/// from the Hugging Face file page so a corrupted/partial download can never
/// be served to the chat pipeline.
pub const MANAGED_MODEL_NAME: &str = "qwen2.5-coder:7b";
pub const MANAGED_MODEL_URL: &str =
    "https://huggingface.co/bartowski/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/Qwen2.5-Coder-7B-Instruct-Q4_K_M.gguf";
pub const MANAGED_MODEL_SHA256: &str =
    "1664fccab734674a50763490a8c6931b70e3f2f8ec10031b54806d30e5f956b6";
pub const MANAGED_MODEL_BYTES: u64 = 4_682_290_816; // ~4.68 GiB (HF page)

/// App data dir layout: <data>/localai/models/<name>.gguf
fn model_store_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .ok_or("cannot resolve app data dir")?
        .join("NexoraGameStudio")
        .join("localai")
        .join("models");
    Ok(base)
}

pub fn managed_model_path() -> Result<PathBuf, String> {
    Ok(model_store_dir()?.join("qwen2.5-coder-7b-instruct-q4_k_m.gguf"))
}

/// Where the sidecar binary lands at runtime. In packaged builds Tauri puts
/// externalBin binaries next to the main executable; in dev this path won't
/// exist, and we fall back to system runtimes instead.
fn bundled_llama_server() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let path = exe_dir.join("llama-server.exe");
    path.is_file().then_some(path)
}

/// Does the app have any embedded llama-server next to its exe?
pub fn has_bundled_engine() -> bool {
    bundled_llama_server().is_some()
}

fn find_ollama_exe() -> Option<PathBuf> {
    if let Some(path) = which_cmd("ollama") {
        return Some(path);
    }
    let home = dirs::home_dir()?;
    let candidates = [
        home.join("AppData").join("Local").join("Programs").join("Ollama").join("ollama.exe"),
        home.join("AppData").join("Local").join("Ollama").join("ollama.exe"),
        PathBuf::from(r"C:\Program Files\Ollama\ollama.exe"),
        home.join(".local").join("bin").join("ollama"),
        PathBuf::from("/usr/local/bin/ollama"),
    ];
    candidates.into_iter().find(|c| c.is_file())
}

fn which_cmd(name: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        for ext in ["", ".exe"] {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAiStatus {
    pub reachable: bool,
    pub endpoint: Option<String>,
    /// running (live server) | bundled (engine present, not started) |
    /// installed (system Ollama found, not running) | none
    pub runtime_found: String,
    /// Sidecar engine shippable with this build.
    pub engine_bundled: bool,
    /// Managed GGUF downloaded + SHA-verified.
    pub model_ready: bool,
    /// Download in progress right now.
    pub model_downloading: bool,
    pub installed_models: Vec<String>,
    pub hint: String,
}

async fn probe(base_url: &str) -> bool {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .connect_timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    client
        .get(&url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Endpoint resolver for chat: try the saved/configured endpoint first, then
/// sweep the known loopback ports. Exposed to llm.rs so sending chat to the
/// Local provider works even when the provider config still carries the
/// Ollama default (11434) while the bundled engine serves on 8080.
pub async fn resolve_local_endpoint(saved_base_url: Option<&str>) -> Option<String> {
    if let Some(saved) = saved_base_url.map(str::trim).filter(|s| !s.is_empty()) {
        let saved = saved.trim_end_matches('/').to_string();
        // Config files have carried both `/v1`-suffixed and bare roots; the
        // probe normalizes by trying the OpenAI path either way.
        let candidates: Vec<String> = if saved.ends_with("/v1") {
            vec![saved.clone()]
        } else {
            vec![format!("{saved}/v1"), saved.clone()]
        };
        for c in &candidates {
            if probe(c).await {
                return Some(c.clone());
            }
        }
    }
    find_running().await.map(str::to_string)
}

async fn find_running() -> Option<&'static str> {
    for endpoint in LOCAL_ENDPOINTS {
        if probe(endpoint).await {
            return Some(endpoint);
        }
    }
    None
}

async fn ollama_models() -> Vec<String> {
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(3)).build() else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct Models { models: Vec<Name> }
    #[derive(serde::Deserialize)]
    struct Name { name: String }
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(resp) => resp
            .json::<Models>()
            .await
            .map(|m| m.models.into_iter().map(|n| n.name).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn model_ready_now() -> bool {
    managed_model_path().is_ok_and(|p| p.is_file())
}

#[tauri::command]
pub async fn local_ai_status(base_url: Option<String>) -> LocalAiStatus {
    let saved = base_url.filter(|s| !s.trim().is_empty());
    let mut hint = String::new();

    let endpoint = if let Some(saved) = saved.as_deref() {
        if probe(saved).await {
            Some(saved.to_string())
        } else {
            find_running().await.map(str::to_string)
        }
    } else {
        find_running().await.map(str::to_string)
    };

    let reachable = endpoint.is_some();
    let mut runtime_found = "none".to_string();
    let mut installed_models = Vec::new();
    if reachable {
        runtime_found = "running".into();
        installed_models = ollama_models().await;
    } else if has_bundled_engine() && model_ready_now() {
        runtime_found = "bundled".into();
        hint = "Zero-setup ready — click Start offline engine.".into();
    } else if has_bundled_engine() {
        runtime_found = "bundled".into();
        hint = "Engine bundled — download the model once (~4.7 GB), then offline forever.".into();
    } else if find_ollama_exe().is_some() {
        runtime_found = "installed".into();
        hint = "Ollama installed but not running — enable and start, or use the bundled engine.".into();
    } else {
        hint = "Click Download to fetch the offline model (~4.7 GB, one time). The engine itself ships with the app.".into();
    }

    LocalAiStatus {
        reachable,
        endpoint,
        runtime_found,
        engine_bundled: has_bundled_engine(),
        model_ready: model_ready_now(),
        model_downloading: super::MODEL_DOWNLOAD_ACTIVE.load(std::sync::atomic::Ordering::SeqCst),
        installed_models,
        hint,
    }
}

/// Stream the managed GGUF to app-data with progress events + SHA-256 check.
/// Writes to `<final>.part` and renames only after digest verification, so a
/// failed/interrupted download never leaves a loadable corrupt model.
#[tauri::command]
pub async fn local_ai_download_model(
    app: tauri::AppHandle,
) -> Result<String, String> {
    if model_ready_now() {
        return Ok("already downloaded".into());
    }
    if super::MODEL_DOWNLOAD_ACTIVE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("download already running".into());
    }
    let result = download_managed_model(&app).await;
    super::MODEL_DOWNLOAD_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    result
}

/// Shared download core (HTTP fetch of the pinned GGUF + verify + rename).
async fn download_managed_model(app: &tauri::AppHandle) -> Result<String, String> {
    use futures_util::StreamExt;
    (async {
        let dir = model_store_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("model dir: {e}"))?;
        let final_path = managed_model_path()?;
        let part_path = dir.join("qwen2.5-coder-7b-instruct-q4_k_m.gguf.part");

        let resp = reqwest::Client::builder()
            .timeout(Duration::from_secs(3600))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("http client: {e}"))?
            .get(MANAGED_MODEL_URL)
            .send()
            .await
            .map_err(|e| format!("model download request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("model download failed: {e}"))?;

        // Prefer the reported size over the pinned estimate when available.
        let total = resp.content_length().unwrap_or(MANAGED_MODEL_BYTES);
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(&part_path).map_err(|e| format!("temp file: {e}"))?,
        );
        let mut hasher = Sha256::new();
        let mut stream = resp.bytes_stream();
        let mut done: u64 = 0;
        let mut last_emit = std::time::Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("download interrupted: {e}"))?;
            hasher.update(&chunk);
            std::io::Write::write_all(&mut file, &chunk)
                .map_err(|e| format!("write failed: {e}"))?;
            done += chunk.len() as u64;
            if last_emit.elapsed() >= Duration::from_millis(500) {
                let _ = app.emit(
                    "llama-model-download",
                    serde_json::json!({
                        "received": done, "total": total,
                        "percent": if total == 0 { 0.0 } else { done as f64 / total as f64 * 100.0 },
                        "done": false,
                    }),
                );
                last_emit = std::time::Instant::now();
            }
        }
        std::io::Write::flush(&mut file).map_err(|e| format!("flush: {e}"))?;
        drop(file);

        let digest = format!("{:x}", hasher.finalize());
        let _ = app.emit(
            "llama-model-download",
            serde_json::json!({ "received": done, "total": total, "percent": 100.0, "done": false, "verifying": true }),
        );
        if !digest.eq_ignore_ascii_case(MANAGED_MODEL_SHA256) {
            let _ = std::fs::remove_file(&part_path);
            return Err(format!(
                "model checksum mismatch — got {digest}, refusing to use it"
            ));
        }
        std::fs::rename(&part_path, &final_path)
            .map_err(|e| format!("finalize failed: {e}"))?;
        let _ = app.emit(
            "llama-model-download",
            serde_json::json!({ "received": done, "total": total, "percent": 100.0, "done": true }),
        );
        Ok(final_path.to_string_lossy().into_owned())
    })
    .await
}

/// Start the offline engine: bundled sidecar with the managed model, else
/// system Ollama. Resolves when an endpoint answers /models.
#[tauri::command]
pub async fn local_ai_start(app: tauri::AppHandle) -> Result<String, String> {
    let _ = &app; // kept for command signature symmetry with local_ai_setup
    // Live server already? Just report it.
    if let Some(endpoint) = find_running().await {
        ensure_model_pulled(endpoint).await;
        return Ok(endpoint.to_string());
    }

    // 1) Bundled sidecar — the zero-setup path (model must already be there).
    if let Some(engine_path) = bundled_llama_server() {
        if model_ready_now() {
            return start_bundled_llama_server(&engine_path).await;
        }
        if find_ollama_exe().is_none() {
            return Err("Engine is bundled but the model isn't downloaded yet — use the guided setup (Download model).".into());
        }
        // no sidecar model + Ollama exists → fall through to Ollama below
    }

    // 2) System Ollama fallback.
    if let Some(ollama) = find_ollama_exe() {
        let mut cmd = std::process::Command::new(&ollama);
        cmd.arg("serve");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn().map_err(|e| format!("failed to start Ollama: {e}"))?;
        if wait_ready(LOCAL_ENDPOINTS[0], Duration::from_secs(30)).await {
            ensure_model_pulled(LOCAL_ENDPOINTS[0]).await;
            return Ok(LOCAL_ENDPOINTS[0].to_string());
        }
        return Err("Ollama started but did not become ready — run `ollama serve` in a terminal to see why".into());
    }

    Err("No offline engine available — the bundled engine requires the packaged app; install Ollama as a fallback.".into())
}

/// Launch the bundled llama-server on the managed model and wait for ready.
async fn start_bundled_llama_server(engine_path: &std::path::Path) -> Result<String, String> {
    let model = managed_model_path()?;
    let mut cmd = std::process::Command::new(engine_path);
    cmd.arg("-m").arg(&model)
        .arg("--host").arg("127.0.0.1")
        .arg("--port").arg("8080")
        .arg("-c").arg("8192")
        .arg("-ngl").arg("0")
        .arg("--no-webui");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("failed to start llama-server: {e}"))?;
    if wait_ready("http://localhost:8080/v1", Duration::from_secs(180)).await {
        return Ok("http://localhost:8080/v1".into());
    }
    Err("llama-server started but did not become ready — check the app log".into())
}

/// One-stage progress event for `local_ai_setup`.
fn emit_stage(app: &tauri::AppHandle, stage: &str, status: &str, detail: &str) {
    let _ = app.emit(
        "local-ai-setup",
        serde_json::json!({ "stage": stage, "status": status, "detail": detail }),
    );
}

/// Guided zero-setup flow — one call that does everything:
///
/// 1. probe for a live runtime (nothing to do → done)
/// 2. ensure a model exists: bundled path = HTTP GGUF download + SHA verify;
///    Ollama path = streamed `api/pull` with progress events
/// 3. start the engine and wait until it answers on loopback
///
/// Every transition emits a `local-ai-setup` event so the UI can narrate the
/// steps. Failures abort with a specific, actionable message.
#[tauri::command]
pub async fn local_ai_setup(app: tauri::AppHandle) -> Result<String, String> {
    emit_stage(&app, "probe", "running", "Looking for an offline engine…");

    // Already serving? Nothing to do.
    if let Some(endpoint) = find_running().await {
        ensure_model_pulled(endpoint).await;
        emit_stage(&app, "ready", "done", endpoint);
        return Ok(endpoint.to_string());
    }

    let engine_bundled = has_bundled_engine();
    let ollama = find_ollama_exe();
    if !engine_bundled && ollama.is_none() {
        emit_stage(&app, "engine", "error", "No offline engine on this machine.");
        return Err("No offline engine is bundled with this build and Ollama was not found — install Ollama (ollama.com/download) or repackage the app with the engine sidecar.".into());
    }

    // ---- Model acquisition -------------------------------------------------
    if engine_bundled && !model_ready_now() {
        emit_stage(&app, "model", "running", "Downloading Qwen2.5-Coder 7B (~4.7 GB, one time)…");
        if super::MODEL_DOWNLOAD_ACTIVE.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err("a model download is already running".into());
        }
        let model_result = download_managed_model(&app).await;
        super::MODEL_DOWNLOAD_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
        match model_result {
            Ok(_path) => {
                emit_stage(&app, "model", "done", "Model verified and ready.");
            }
            Err(e) => {
                let msg = format!("Model download failed: {e}");
                emit_stage(&app, "model", "error", &msg);
                return Err(msg);
            }
        }
    }

    // ---- Start the engine --------------------------------------------------
    emit_stage(&app, "engine", "running", "Starting the offline engine…");
    let endpoint = if engine_bundled && model_ready_now() {
        let engine_path = bundled_llama_server().expect("checked above");
        emit_stage(&app, "engine", "running", "Starting bundled llama-server with the downloaded model (first start loads ~4.7 GB into memory, be patient)…");
        match start_bundled_llama_server(&engine_path).await {
            Ok(endpoint) => endpoint,
            Err(e) => {
                let msg = format!("Bundled engine failed: {e}");
                emit_stage(&app, "engine", "error", &msg);
                return Err(msg);
            }
        }
    } else if let Some(ollama) = ollama {
        emit_stage(&app, "engine", "running", "Starting Ollama…");
        let mut cmd = std::process::Command::new(&ollama);
        cmd.arg("serve");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        if let Err(e) = cmd.spawn() {
            let msg = format!("failed to start Ollama: {e}");
            emit_stage(&app, "engine", "error", &msg);
            return Err(msg);
        }
        if !wait_ready(LOCAL_ENDPOINTS[0], Duration::from_secs(30)).await {
            let msg = "Ollama started but did not become ready — run `ollama serve` in a terminal to see why".to_string();
            emit_stage(&app, "engine", "error", &msg);
            return Err(msg);
        }
        // Stream the model pull (Ollama registry) with progress events.
        if !model_ready_via_ollama().await {
            emit_stage(&app, "model", "running", "Pulling qwen2.5-coder:7b through Ollama (~4.7 GB)…");
            if let Err(e) = ollama_pull_streamed(&app).await {
                let msg = format!("Ollama model pull failed: {e}");
                emit_stage(&app, "model", "error", &msg);
                return Err(msg);
            }
            emit_stage(&app, "model", "done", "qwen2.5-coder:7b pulled.");
        } else {
            emit_stage(&app, "model", "done", "Model already present in Ollama.");
        }
        LOCAL_ENDPOINTS[0].to_string()
    } else {
        unreachable!("engine or ollama checked above");
    };

    emit_stage(&app, "ready", "done", &endpoint);
    Ok(endpoint)
}

/// Ollama registry pull of the managed model. Streams the JSONL progress the
/// Ollama API emits and optionally forwards each update through `on_progress`
/// (None = passive, errors/progress only logged).
async fn ollama_pull_impl<F>(on_progress: Option<F>) -> Result<(), String>
where
    F: Fn(&serde_json::Value),
{
    use futures_util::StreamExt;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .post("http://localhost:11434/api/pull")
        .json(&serde_json::json!({ "name": "qwen2.5-coder:7b", "stream": true }))
        .send()
        .await
        .map_err(|e| format!("pull request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Ollama rejected the pull: {e}"))?;

    let mut stream = resp.bytes_stream();
    let mut buffer = Vec::new();
    let mut last_emit = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("pull stream interrupted: {e}"))?;
        buffer.extend_from_slice(&chunk);
        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let Ok(text) = std::str::from_utf8(&line) else { continue };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { continue };
            if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
                return Err(format!("Ollama pull error: {err}"));
            }
            if last_emit.elapsed() >= Duration::from_millis(700) {
                if let Some(callback) = &on_progress {
                    callback(&value);
                }
                last_emit = std::time::Instant::now();
            }
        }
    }
    tracing::info!("qwen2.5-coder:7b pulled through Ollama");
    Ok(())
}

/// Guided-setup variant: forwards Ollama pull progress into
/// `llama-model-download` events so the same progress bar shows either path.
async fn ollama_pull_streamed(app: &tauri::AppHandle) -> Result<(), String> {
    ollama_pull_impl(Some(move |value: &serde_json::Value| {
        let total = value.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        let completed = value.get("completed").and_then(|v| v.as_u64()).unwrap_or(0);
        let percent = if total == 0 { 0.0 } else { completed as f64 / total as f64 * 100.0 };
        let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let _ = app.emit(
            "llama-model-download",
            serde_json::json!({
                "received": completed, "total": total, "percent": percent,
                "done": false,
                "status": status,
            }),
        );
    }))
    .await?;
    let _ = app.emit(
        "llama-model-download",
        serde_json::json!({ "percent": 100.0, "done": true }),
    );
    Ok(())
}

/// If the endpoint is Ollama, make sure the managed model tag exists, auto-
/// pulling it when missing (one-time, uses Ollama's own model registry).
async fn ensure_model_pulled(endpoint: &str) {
    if !endpoint.starts_with("http://localhost:11434") {
        return;
    }
    if model_ready_via_ollama().await {
        return;
    }
    // Passive auto-pull (no UI events): one attempt, errors only logged.
    if let Err(e) = ollama_pull_impl::<fn(&serde_json::Value)>(None).await {
        tracing::warn!(error = %e, "passive ollama pull failed");
    }
}

/// Is `qwen2.5-coder:7b` already present in the local Ollama?
async fn model_ready_via_ollama() -> bool {
    ollama_models()
        .await
        .iter()
        .any(|m| m.starts_with("qwen2.5-coder:7b"))
}

async fn wait_ready(base_url: &str, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if probe(base_url).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

//! Discovery + lifecycle of the offline local AI runtime (Ollama /
//! llama.cpp `llama-server` / LM Studio).
//!
//! The chat path itself is provider-agnostic (OpenAI-compatible HTTP on
//! localhost, see `llm.rs::call_local`); this module only answers two
//! questions for the UI:
//!
//! 1. *"Is a local runtime reachable right now?"* — probe the well-known
//!    loopback endpoints (and any saved custom base URL).
//! 2. *"Can I start one?"* — if Ollama is installed but not running, spawn
//!    `ollama serve` (Windows: the exe in %LOCALAPPDATA%). Bundled
//!    llama-server sidecars are a build-time option (`bundled()`); the app
//!    does not download anything by itself.
//!
//! Everything here is loopback-only. No cloud, no telemetry, no keys.

use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// Candidate loopback base URLs probed for a local OpenAI-compatible server.
/// Ordered: Ollama first (most common), then llama.cpp, LM Studio, vLLM.
pub const LOCAL_ENDPOINTS: [&str; 5] = [
    "http://localhost:11434/v1", // Ollama
    "http://localhost:8080/v1",  // llama.cpp llama-server
    "http://localhost:1234/v1",  // LM Studio
    "http://localhost:8000/v1",  // vLLM
    "http://localhost:1337/v1",  // llama-server alt / whatever the user runs
];

/// Where a bundled llama-server sidecar would live, if the installer ships one.
/// `resources/llama/llama-server.exe` next to the app binary.
fn bundled_llama_server() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?;
    let dir = exe_dir.parent()?.join("llama");
    let path = dir.join("llama-server.exe");
    path.is_file().then_some(path)
}

/// Locate the Ollama executable on this machine (doesn't need to be running).
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

/// `which`-style lookup over PATH, done manually so we don't need the `which`
/// crate just for this.
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
    /// A local OpenAI-compatible server answered on one of the known endpoints.
    pub reachable: bool,
    /// The endpoint that answered (loopback, includes /v1).
    pub endpoint: Option<String>,
    /// How the runtime was located: running, installed-but-stopped, or bundled.
    pub runtime_found: String,
    /// Ollama model tags this machine already has downloaded (best effort).
    pub installed_models: Vec<String>,
    /// Human guidance for the settings card.
    pub hint: String,
}

/// Quick GET with a tight timeout — probing loopback must fail fast.
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

/// Check every known loopback endpoint for a live OpenAI-compatible server.
async fn find_running() -> Option<&'static str> {
    for endpoint in LOCAL_ENDPOINTS {
        if probe(endpoint).await {
            return Some(endpoint);
        }
    }
    None
}

/// Best-effort list of models already pulled in a running Ollama.
async fn ollama_models() -> Vec<String> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct Models {
        models: Vec<Name>,
    }
    #[derive(serde::Deserialize)]
    struct Name {
        name: String,
    }
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(resp) => resp
            .json::<Models>()
            .await
            .map(|m| m.models.into_iter().map(|n| n.name).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Full status probe used by the AI Settings card and Dashboard badge.
#[tauri::command]
pub async fn local_ai_status(base_url: Option<String>) -> LocalAiStatus {
    // A user-saved endpoint is probed first, then the well-known list.
    let saved = base_url.filter(|s| !s.trim().is_empty());
    let mut runtime_found = "none".to_string();
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
    let mut installed_models = Vec::new();
    if reachable {
        runtime_found = "running".into();
        installed_models = ollama_models().await;
    } else if bundled_llama_server().is_some() {
        runtime_found = "bundled".into();
        hint = "The app ships an offline engine — enable it in AI Settings to start it."
            .into();
    } else if find_ollama_exe().is_some() {
        runtime_found = "installed".into();
        hint = "Ollama is installed but not running. Enable offline local model to start it."
            .into();
    } else {
        hint = "No local engine found. Install Ollama (ollama.com) or LM Studio, or place \
                llama-server.exe in the app's llama/ folder — then enable offline local model."
            .into();
    }

    LocalAiStatus {
        reachable,
        endpoint,
        runtime_found,
        installed_models,
        hint,
    }
}

/// Start the offline runtime: Ollama (detached) or the bundled llama-server.
/// Returns when the endpoint starts answering, or the failure reason.
#[tauri::command]
pub async fn local_ai_start() -> Result<String, String> {
    let endpoint = if probe(LOCAL_ENDPOINTS[0]).await {
        LOCAL_ENDPOINTS[0].to_string()
    } else {
        LOCAL_ENDPOINTS.first().copied().unwrap().to_string()
    };

    // 1) Bundled sidecar takes precedence (fully offline-by-construction).
    if let Some(llama) = bundled_llama_server() {
        let model_dir = llama.parent().unwrap().join("models");
        let mut cmd = std::process::Command::new(&llama);
        if model_dir.is_dir() {
            cmd.arg("--models-dir").arg(&model_dir);
        }
        cmd.arg("--host").arg("127.0.0.1").arg("--port").arg("8080");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
            .map_err(|e| format!("failed to start bundled llama-server: {e}"))?;
        if wait_ready("http://localhost:8080/v1", Duration::from_secs(30)).await {
            return Ok("http://localhost:8080/v1".into());
        }
        return Err("bundled llama-server started but did not become ready".into());
    }

    // 2) System Ollama.
    if let Some(ollama) = find_ollama_exe() {
        let mut cmd = std::process::Command::new(&ollama);
        cmd.arg("serve");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
            .map_err(|e| format!("failed to start Ollama: {e}"))?;
        if wait_ready(LOCAL_ENDPOINTS[0], Duration::from_secs(30)).await {
            return Ok(LOCAL_ENDPOINTS[0].to_string());
        }
        return Err("Ollama started but did not become ready — check `ollama serve` in a terminal".into());
    }

    let _ = endpoint;
    Err("No local AI engine available. Install Ollama (ollama.com) or place llama-server.exe in the app's llama/ folder.".into())
}

/// Poll an endpoint until it answers /models, up to `timeout`.
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

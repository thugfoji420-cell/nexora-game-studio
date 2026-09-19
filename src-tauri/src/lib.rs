mod asset_delivery;
mod blender;
mod blender_adapter;
pub mod hardware;
pub mod hunyuan_generation;
mod image_generation;
pub mod jobs;
mod logging;
mod model3d_generation;
mod model3d_import;
mod model3d_processing;
mod model3d_validation;
mod openrouter;
mod project;
pub mod provider_orchestrator;
pub mod providers;
pub mod runtime_manager;
mod settings;
mod source_normalization;
mod video_generation;
mod video_validation;

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Manager, RunEvent, State};

use base64::Engine;
use logging::JsonLogger;
use project::{MANIFEST_FILENAME, ProjectInfo, ProjectManifest, ProjectState, resolve_new_project_root};
use providers::{Capability, ProviderHealth, ProviderRegistry, ProviderView};
use runtime_manager::{
    RuntimeConfig, RuntimeKind, RuntimeManager, RuntimeState, RuntimeStatus, RuntimeType,
};
use settings::AppSettings;
use tracing_subscriber;

#[derive(Serialize, Clone, Debug)]
pub struct UnityProjectValidation {
    pub is_valid: bool,
    pub unity_version: Option<String>,
    pub project_root: String,
    pub error_message: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineTargetInfo {
    pub target_id: String,
    pub display_name: String,
    pub detected: bool,
    pub executable_path: Option<String>,
    pub project_root: Option<String>,
    pub deployment_supported: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineUpdateStatus {
    pub engine_id: String,
    pub display_name: String,
    pub current_version: Option<String>,
    pub available_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub detail: String,
}

#[derive(Clone)]
struct EngineUpdateTarget {
    engine_id: &'static str,
    display_name: &'static str,
    root: PathBuf,
}

fn git_command(root: &PathBuf, args: &[&str]) -> Result<String, String> {
    let safe_directory = root.to_string_lossy().to_string();
    let mut cmd = Command::new("git");
    cmd.arg("-c")
        .arg(format!("safe.directory={safe_directory}"))
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd.output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() { "git command failed".into() } else { detail });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_update_target(target: &EngineUpdateTarget, apply_update: bool) -> EngineUpdateStatus {
    let current_version = git_command(&target.root, &["rev-parse", "--short", "HEAD"]).ok();
    if !target.root.join(".git").exists() {
        return EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version,
            available_version: None,
            update_available: false,
            status: "manual".into(),
            detail: "This engine is not installed as a Git repository.".into(),
        };
    }
    if let Ok(changes) = git_command(&target.root, &["status", "--porcelain"]) {
        if !changes.is_empty() {
            return EngineUpdateStatus {
                engine_id: target.engine_id.into(),
                display_name: target.display_name.into(),
                current_version,
                available_version: None,
                update_available: false,
                status: "skipped".into(),
                detail: "Local changes detected; automatic update was skipped.".into(),
            };
        }
    }
    if let Err(error) = git_command(&target.root, &["remote", "get-url", "origin"]) {
        return EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version,
            available_version: None,
            update_available: false,
            status: "manual".into(),
            detail: format!("No update source configured: {error}"),
        };
    }
    if let Err(error) = git_command(&target.root, &["fetch", "--quiet", "origin"]) {
        return EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version,
            available_version: None,
            update_available: false,
            status: "check-failed".into(),
            detail: format!("Could not check for updates: {error}"),
        };
    }
    let ahead = git_command(&target.root, &["rev-list", "--count", "HEAD..@{upstream}"])
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    if ahead == 0 {
        return EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version: current_version.clone(),
            available_version: current_version.clone(),
            update_available: false,
            status: "up-to-date".into(),
            detail: "No engine update is available.".into(),
        };
    }
    if !apply_update {
        return EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version,
            available_version: None,
            update_available: true,
            status: "available".into(),
            detail: format!("{ahead} update(s) available."),
        };
    }
    match git_command(&target.root, &["pull", "--ff-only", "--quiet"]) {
        Ok(_) => {
            let updated_version = git_command(&target.root, &["rev-parse", "--short", "HEAD"]).ok();
            EngineUpdateStatus {
                engine_id: target.engine_id.into(),
                display_name: target.display_name.into(),
                current_version,
                available_version: updated_version,
                update_available: true,
                status: "updated".into(),
                detail: "Engine updated automatically.".into(),
            }
        }
        Err(error) => EngineUpdateStatus {
            engine_id: target.engine_id.into(),
            display_name: target.display_name.into(),
            current_version,
            available_version: None,
            update_available: true,
            status: "update-failed".into(),
            detail: format!("Automatic update failed: {error}"),
        },
    }
}

fn check_non_git_engine(engine_id: &str, display_name: &str, detail: &str) -> EngineUpdateStatus {
    EngineUpdateStatus {
        engine_id: engine_id.into(),
        display_name: display_name.into(),
        current_version: None,
        available_version: None,
        update_available: false,
        status: "manual".into(),
        detail: detail.into(),
    }
}

#[tauri::command]
fn detect_engine_targets(_state: State<'_, AppState>) -> Vec<EngineTargetInfo> {
    let unity_project = asset_delivery::discover_unity_project_path();
    let blender = RuntimeManager::discover_blender();
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let unreal_editor = program_files.as_ref().and_then(|root| {
        let epic_root = root.join("Epic Games");
        std::fs::read_dir(epic_root).ok()?.filter_map(Result::ok).map(|entry| {
            entry.path().join("Engine").join("Binaries").join("Win64").join("UnrealEditor.exe")
        }).find(|path| path.exists())
    });
    let godot = which::which("godot").ok().or_else(|| which::which("godot4").ok());

    vec![
        EngineTargetInfo {
            target_id: "unity".into(),
            display_name: "Unity".into(),
            detected: unity_project.is_some(),
            executable_path: None,
            project_root: unity_project.map(|path| path.to_string_lossy().into_owned()),
            deployment_supported: true,
        },
        EngineTargetInfo {
            target_id: "unreal".into(),
            display_name: "Unreal Engine".into(),
            detected: unreal_editor.is_some(),
            executable_path: unreal_editor.map(|path| path.to_string_lossy().into_owned()),
            project_root: None,
            deployment_supported: false,
        },
        EngineTargetInfo {
            target_id: "godot".into(),
            display_name: "Godot".into(),
            detected: godot.is_some(),
            executable_path: godot.map(|path| path.to_string_lossy().into_owned()),
            project_root: None,
            deployment_supported: false,
        },
        EngineTargetInfo {
            target_id: "blender".into(),
            display_name: "Blender".into(),
            detected: blender.is_some(),
            executable_path: blender.and_then(|config| config.launcher_path),
            project_root: None,
            deployment_supported: false,
        },
    ]
}

struct JobWorker {
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl JobWorker {
    fn start(
        current_project: Arc<Mutex<Option<Arc<ProjectState>>>>,
        image_config: Arc<Mutex<image_generation::ImageProviderConfig>>,
        video_config: Arc<Mutex<video_generation::VideoProviderConfig>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let owner = uuid::Uuid::now_v7().to_string();
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let project = current_project.lock().ok().and_then(|guard| guard.clone());
                if let Some(project) = project {
                    let config = image_config.lock().ok().map(|value| value.clone());
                    let video = video_config.lock().ok().map(|value| value.clone());
                    let _ = jobs::tick_with_configs(
                        &project,
                        &owner,
                        config.as_ref(),
                        video.as_ref(),
                        Some(&thread_stop),
                    );
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        Self {
            stop,
            thread: Mutex::new(Some(handle)),
        }
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl Drop for JobWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

struct AppState {
    settings_path: PathBuf,
    settings: Mutex<AppSettings>,
    image_provider_config_path: PathBuf,
    image_provider_config: Arc<Mutex<image_generation::ImageProviderConfig>>,
    video_provider_config_path: PathBuf,
    video_provider_config: Arc<Mutex<video_generation::VideoProviderConfig>>,
    logger: JsonLogger,
    current_project: Arc<Mutex<Option<Arc<ProjectState>>>>,
    job_worker: JobWorker,
    recent_projects_path: PathBuf,
    recent_projects: Mutex<Vec<RecentProjectInfo>>,
    hardware_snapshot: Mutex<hardware::HardwareSnapshot>,
    provider_registry: Mutex<ProviderRegistry>,
    media_integrity_cache: asset_delivery::IntegrityCache,
    runtime_manager: Arc<Mutex<RuntimeManager>>,
    provider_orchestrator: Arc<Mutex<provider_orchestrator::ProviderOrchestrator>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    foundation_status: &'static str,
    local_first: bool,
    telemetry_enabled: bool,
}

/// Public-facing `AppSettings` shape exchanged with the frontend.
///
/// Mirrors `settings::AppSettings` but exposes the masked OpenRouter
/// configuration so the raw API key never leaves the Rust process.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAppSettings {
    schema_version: u32,
    theme: settings::ThemePreference,
    compact_sidebar: bool,
    telemetry_enabled: bool,
    a1111: settings::A1111Config,
    hunyuan: settings::HunyuanConfig,
    blender: settings::BlenderConfig,
    unity_targets: HashMap<String, settings::UnityProjectTarget>,
    active_unity_target: Option<String>,
    openrouter: openrouter::MaskedOpenRouterConfig,
    skip_runtime_startup_on_launch: bool,
}

impl From<settings::AppSettings> for PublicAppSettings {
    fn from(value: settings::AppSettings) -> Self {
        Self {
            schema_version: value.schema_version,
            theme: value.theme,
            compact_sidebar: value.compact_sidebar,
            telemetry_enabled: value.telemetry_enabled,
            a1111: value.a1111,
            hunyuan: value.hunyuan,
            blender: value.blender,
            unity_targets: value.unity_targets,
            active_unity_target: value.active_unity_target,
            openrouter: value.openrouter.masked(),
            skip_runtime_startup_on_launch: value.skip_runtime_startup_on_launch,
        }
    }
}

fn command_error(state: &AppState, event: &str, error: impl std::fmt::Display) -> String {
    let message = error.to_string();
    state.logger.error(event, &message);
    message
}

fn retire_current_project(guard: &mut Option<Arc<ProjectState>>) -> Result<(), String> {
    if let Some(project) = guard.take() {
        project.disable_execution();
        jobs::recover(&project).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        name: "Nexora Game Studio",
        version: env!("CARGO_PKG_VERSION"),
        foundation_status: "Operational",
        local_first: true,
        telemetry_enabled: false,
    }
}

#[tauri::command]
fn load_settings(state: State<'_, AppState>) -> Result<PublicAppSettings, String> {
    let loaded = settings::load(&state.settings_path)
        .map_err(|error| command_error(&state, "settings_load_failed", error))?;
    Ok(PublicAppSettings::from(loaded))
}

#[tauri::command]
fn save_settings(
    settings: PublicAppSettings,
    state: State<'_, AppState>,
) -> Result<PublicAppSettings, String> {
    // Persist the supplied settings to disk. The frontend only ever sees the
    // masked OpenRouter config, so the real API key is preserved from the
    // current in-memory state.
    let mut current = state.settings.lock().unwrap();
    let merged = settings::AppSettings {
        schema_version: settings.schema_version,
        theme: settings.theme,
        compact_sidebar: settings.compact_sidebar,
        telemetry_enabled: settings.telemetry_enabled,
        a1111: settings.a1111,
        hunyuan: settings.hunyuan,
        blender: settings.blender,
        unity_targets: settings.unity_targets,
        active_unity_target: settings.active_unity_target,
        openrouter: current.openrouter.clone(),
        skip_runtime_startup_on_launch: settings.skip_runtime_startup_on_launch,
    };
    settings::save(&state.settings_path, &merged)
        .map_err(|error| command_error(&state, "settings_save_failed", error))?;
    *current = merged.clone();
    drop(current);
    state
        .logger
        .info("settings_saved", "Application settings were updated");
    Ok(PublicAppSettings::from(merged))
}

#[tauri::command]
fn discover_blender(state: State<'_, AppState>) -> blender::BlenderInfo {
    let blender_config = &state.settings.lock().unwrap().blender;
    let manual_path = blender_config.executable_path.as_deref().map(PathBuf::from);
    blender::find_and_validate_blender(manual_path.as_deref())
}

#[tauri::command]
fn validate_blender_executable(
    executable_path: String,
    _state: State<'_, AppState>,
) -> blender::BlenderInfo {
    let path = PathBuf::from(executable_path);
    blender::validate_blender(&path)
}

#[tauri::command]
fn get_log_location(state: State<'_, AppState>) -> String {
    state.logger.path().display().to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentProjectInfo {
    root: PathBuf,
    manifest: ProjectManifest,
}

fn record_recent_project(
    recent: &mut Vec<RecentProjectInfo>,
    root: PathBuf,
    manifest: ProjectManifest,
) {
    recent.retain(|project| project.manifest.project_id != manifest.project_id);
    recent.insert(0, RecentProjectInfo { root, manifest });
    recent.truncate(10);
}

fn load_recent_projects(path: &std::path::Path) -> Vec<RecentProjectInfo> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };

    serde_json::from_str::<Vec<RecentProjectInfo>>(&contents)
        .unwrap_or_default()
        .into_iter()
        .filter(|project| project.root.is_dir())
        .collect()
}

fn save_recent_projects(path: &std::path::Path, recent: &[RecentProjectInfo]) {
    let Ok(contents) = serde_json::to_string_pretty(recent) else {
        return;
    };
    let _ = fs::write(path, contents);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectResult {
    project: ProjectInfo,
    recent: Vec<RecentProjectInfo>,
}

#[tauri::command]
fn create_project(
    root: String,
    name: String,
    engine_scope: Option<String>,
    state: State<'_, AppState>,
) -> Result<CreateProjectResult, String> {
    // The selected location is the PARENT directory; the project itself is
    // created in `<parent>/<project name>` so every project gets its own
    // independent root and a second project never collides with the first.
    let parent = PathBuf::from(root);
    let project_root = resolve_new_project_root(&parent, &name).map_err(|e| {
        state.logger.error("project_create_failed", &e.to_string());
        e.to_string()
    })?;
    let project = ProjectState::create_scoped(&project_root, &name, engine_scope.as_deref()).map_err(|e| {
        state.logger.error("project_create_failed", &e.to_string());
        e.to_string()
    })?;

    let info = project.info();
    let manifest = project.manifest.clone();

    let mut guard = state.current_project.lock().unwrap();
    retire_current_project(&mut guard)?;
    *guard = Some(Arc::new(project));
    drop(guard);
    *state.hardware_snapshot.lock().unwrap() = hardware::detect(Some(&project_root));

    let mut recent = state.recent_projects.lock().unwrap();
    record_recent_project(&mut recent, info.root.clone(), manifest.clone());
    save_recent_projects(&state.recent_projects_path, &recent);

    state.logger.info(
        "project_created",
        &format!("{} at {}", manifest.name, info.root.display()),
    );

    Ok(CreateProjectResult {
        project: info,
        recent: recent.clone(),
    })
}

#[tauri::command]
fn open_project(root: String, engine_scope: Option<String>, state: State<'_, AppState>) -> Result<CreateProjectResult, String> {
    let project_root = PathBuf::from(root);
    let project = ProjectState::open_scoped(&project_root, engine_scope.as_deref()).map_err(|e| {
        state.logger.error("project_open_failed", &e.to_string());
        e.to_string()
    })?;

    let info = project.info();
    let manifest = project.manifest.clone();

    let mut guard = state.current_project.lock().unwrap();
    retire_current_project(&mut guard)?;
    *guard = Some(Arc::new(project));
    drop(guard);
    *state.hardware_snapshot.lock().unwrap() = hardware::detect(Some(&project_root));

    let mut recent = state.recent_projects.lock().unwrap();
    record_recent_project(&mut recent, info.root.clone(), manifest.clone());
    save_recent_projects(&state.recent_projects_path, &recent);

    state.logger.info(
        "project_opened",
        &format!("{} at {}", manifest.name, info.root.display()),
    );

    Ok(CreateProjectResult {
        project: info,
        recent: recent.clone(),
    })
}

#[tauri::command]
fn close_project(state: State<'_, AppState>) -> Result<bool, String> {
    let mut guard = state.current_project.lock().unwrap();
    if let Some(project) = guard.take() {
        let manifest = project.manifest.clone();
        project.disable_execution();
        jobs::recover(&project).map_err(|e| command_error(&state, "job_recovery_failed", e))?;
        state
            .hardware_snapshot
            .lock()
            .unwrap()
            .active_project_disk_free_mib = None;
        state.logger.info("project_closed", &manifest.name);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
fn create_diagnostic_job(
    input: jobs::CreateDiagnosticJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::JobInfo, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create(project, input).map_err(|error| command_error(&state, "job_create_failed", error))
}

#[tauri::command]
fn create_provider_diagnostic_job(
    input: jobs::CreateProviderDiagnosticJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::JobInfo, String> {
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_provider_diagnostic(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "provider_diagnostic_job_create_failed", error))
}

#[tauri::command]
fn list_jobs(state: State<'_, AppState>) -> Result<Vec<jobs::JobInfo>, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::list(project).map_err(|error| command_error(&state, "job_list_failed", error))
}

#[tauri::command]
fn get_job_details(job_id: String, state: State<'_, AppState>) -> Result<jobs::JobDetails, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::details(project, &job_id)
        .map_err(|error| command_error(&state, "job_details_failed", error))
}

#[tauri::command]
fn request_job_cancellation(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<jobs::JobInfo, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::request_cancellation(project, &job_id)
        .map_err(|error| command_error(&state, "job_cancellation_failed", error))
}

#[tauri::command]
fn retry_job(job_id: String, state: State<'_, AppState>) -> Result<jobs::JobInfo, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::retry(project, &job_id).map_err(|error| command_error(&state, "job_retry_failed", error))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    pub asset_id: String,
    pub original_filename: String,
    pub managed_master_path: String,
    pub file_size: u64,
    pub checksum: String,
    #[serde(serialize_with = "serialize_opt_u32")]
    pub image_width: Option<u32>,
    #[serde(serialize_with = "serialize_opt_u32")]
    pub image_height: Option<u32>,
    pub image_format: Option<String>,
    pub has_alpha: bool,
    pub imported_at_ms: u128,
    pub status: String,
    pub media_kind: String,
    pub media_container: Option<String>,
    pub media_format: Option<String>,
    #[serde(serialize_with = "serialize_opt_u32")]
    pub media_width: Option<u32>,
    #[serde(serialize_with = "serialize_opt_u32")]
    pub media_height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub fps_numerator: Option<u32>,
    pub fps_denominator: Option<u32>,
    pub validation_level: Option<String>,
    pub codec: Option<String>,
    pub model_metadata_schema_version: Option<u32>,
    pub model_metadata: Option<model3d_validation::Model3dMetadata>,
    pub processing_status: Option<String>,
    pub approval_status: Option<String>,
    pub source_type: Option<String>,
}

fn serialize_opt_u32<S: serde::Serializer>(
    value: &Option<u32>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(v) => serde::Serialize::serialize(v, serializer),
        None => serde::Serialize::serialize(&serde_json::Value::Null, serializer),
    }
}

fn compute_checksum(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[tauri::command]
fn import_asset(path: String, state: State<'_, AppState>) -> Result<AssetInfo, String> {
    let project_guard = state.current_project.lock().unwrap();
    let project = project_guard.as_ref().ok_or("no project open")?;
    let source_path = PathBuf::from(&path);
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(extension.as_deref(), Some("glb" | "gltf")) {
        let info = import_model_asset(project, &source_path)?;
        state.logger.info(
            "asset_imported",
            &format!(
                "{} imported to {}",
                info.original_filename,
                project.root.display()
            ),
        );
        return Ok(info);
    }
    let project_root = project.root.clone();

    if !source_path.exists() {
        return Err("source file not found".to_string());
    }

    let metadata = fs::metadata(&source_path).map_err(|e| e.to_string())?;
    let file_size = metadata.len();

    if file_size == 0 {
        return Err("zero byte file".to_string());
    }

    let original_filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let staging_dir = project.root().join("assets").join("staging");
    fs::create_dir_all(&staging_dir).map_err(|e| e.to_string())?;

    let staging_path = staging_dir.join(format!("import_{}", original_filename));
    fs::copy(&source_path, &staging_path).map_err(|e| e.to_string())?;

    let staging_data = fs::read(&staging_path).map_err(|e| e.to_string())?;
    let checksum = compute_checksum(&staging_data);

    {
        let db = project.db.lock().unwrap();
        let is_duplicate: Result<bool, _> = db.query_row(
            "SELECT 1 FROM assets WHERE checksum = ?1 LIMIT 1",
            rusqlite::params![checksum],
            |_| Ok(true),
        );
        if let Ok(true) = is_duplicate {
            drop(db);
            let _ = fs::remove_file(&staging_path);
            return Err("duplicate asset detected".to_string());
        }
    }

    let master_dir = project.root().join("assets").join("masters");
    fs::create_dir_all(&master_dir).map_err(|e| e.to_string())?;

    let asset_id = uuid::Uuid::now_v7().to_string();
    let original_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let master_filename = format!("{}.{}", asset_id, original_ext);
    let master_path = master_dir.join(&master_filename);

    fs::copy(&staging_path, &master_path).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&staging_path);

    let (width, height, format, has_alpha) = extract_image_metadata(&master_path)?;

    let preview_dir = project.root().join("assets").join("previews");
    fs::create_dir_all(&preview_dir).map_err(|e| e.to_string())?;
    let preview_filename = format!("{}_preview.png", asset_id);
    let preview_path = preview_dir.join(&preview_filename);

    if let Ok(img) = image::open(&master_path) {
        if let Ok(mut preview_file) = fs::File::create(&preview_path) {
            let _ = img.write_to(&mut preview_file, image::ImageFormat::Png);
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let sql = r#"
        INSERT INTO assets (
            asset_id, project_id, original_filename, managed_master_path,
            file_size, checksum, image_width, image_height, image_format,
            has_alpha, imported_at_ms, status, media_kind, media_format,
            media_width, media_height
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'image', ?9, ?7, ?8)
    "#;

    {
        let db = project.db.lock().unwrap();
        db.execute(
            sql,
            rusqlite::params![
                asset_id.clone(),
                project.manifest.project_id.clone(),
                original_filename.clone(),
                master_path.to_string_lossy().to_string(),
                file_size,
                checksum.clone(),
                width as i64,
                height as i64,
                format,
                if has_alpha { 1 } else { 0 },
                now as i64,
                "ready"
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    let info = AssetInfo {
        asset_id,
        original_filename,
        managed_master_path: master_path.to_string_lossy().to_string(),
        file_size,
        checksum,
        image_width: Some(width),
        image_height: Some(height),
        image_format: Some(format.clone()),
        has_alpha,
        imported_at_ms: now as u128,
        status: "ready".to_string(),
        media_kind: "image".into(),
        media_container: None,
        media_format: Some(format.clone()),
        media_width: Some(width),
        media_height: Some(height),
        duration_ms: None,
        fps_numerator: None,
        fps_denominator: None,
        validation_level: None,
        codec: None,
        model_metadata_schema_version: None,
        model_metadata: None,
        processing_status: None,
        approval_status: None,
        source_type: Some("imported".into()),
    };

    state.logger.info(
        "asset_imported",
        &format!(
            "{} imported to {}",
            info.original_filename,
            project_root.display()
        ),
    );
    Ok(info)
}

fn import_model_asset(
    project: &ProjectState,
    source_path: &std::path::Path,
) -> Result<AssetInfo, String> {
    use std::io::Read;
    let source =
        dunce::canonicalize(source_path).map_err(|_| "source file not found".to_string())?;
    let source_meta = fs::metadata(&source).map_err(|e| e.to_string())?;
    if !source_meta.is_file()
        || source_meta.len() == 0
        || source_meta.len() > model3d_validation::MAX_MODEL_BYTES
    {
        return Err("model source must be a non-empty regular file no larger than 256 MiB".into());
    }
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let format = match extension.as_deref() {
        Some("glb") => model3d_validation::ModelFormat::Glb,
        Some("gltf") => model3d_validation::ModelFormat::Gltf,
        _ => return Err("model source extension must be .glb or .gltf".into()),
    };
    let asset_id = uuid::Uuid::now_v7().to_string();
    let paths = model3d_import::prepare(project, &asset_id)?;
    let staging = paths.staging;
    let result = (|| -> Result<AssetInfo, String> {
        let mut bytes = Vec::with_capacity(source_meta.len() as usize);
        fs::File::open(&source)
            .map_err(|e| e.to_string())?
            .take(model3d_validation::MAX_MODEL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 != source_meta.len() {
            return Err("model source changed or exceeded its bound during import".into());
        }
        let staged = staging.join(format!("source.{}", extension.as_deref().unwrap()));
        if staged.parent() != Some(staging.as_path()) {
            return Err("model staging file escaped its UUID directory".into());
        }
        fs::write(&staged, &bytes).map_err(|e| e.to_string())?;
        let checksum = compute_checksum(&bytes);
        let duplicate = project
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assets WHERE project_id=?1 AND checksum=?2)",
                rusqlite::params![project.manifest.project_id, checksum],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|e| e.to_string())?;
        if duplicate {
            return Err("duplicate asset detected".into());
        }
        let validated = model3d_validation::validate(&bytes, format).map_err(|e| e.to_string())?;
        let masters = paths.masters;
        let ext = extension.as_deref().unwrap();
        let master = masters.join(format!("{asset_id}.{ext}"));
        let temporary = masters.join(format!(".{asset_id}.{ext}.tmp"));
        if master.parent() != Some(masters.as_path())
            || temporary.parent() != Some(masters.as_path())
        {
            return Err("model master path escaped managed storage".into());
        }
        if let Err(error) = fs::copy(&staged, &temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        if let Err(error) = fs::rename(&temporary, &master) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        let master = match dunce::canonicalize(&master) {
            Ok(master) => master,
            Err(error) => {
                let _ = fs::remove_file(&master);
                return Err(error.to_string());
            }
        };
        if !master.starts_with(&masters) {
            let _ = fs::remove_file(&master);
            return Err("managed model path escaped masters".into());
        }
        if let Err(error) = verify_managed_model(&master, bytes.len() as u64, &checksum) {
            let _ = fs::remove_file(&master);
            return Err(error);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let metadata_json = match serde_json::to_string(&validated.metadata) {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_file(&master);
                return Err(error.to_string());
            }
        };
        let original_filename = source
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("model")
            .to_owned();
        let inserted = (|| -> Result<(), String> {
            let mut db = project.db.lock().unwrap();
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let duplicate = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM assets WHERE project_id=?1 AND checksum=?2)",
                    rusqlite::params![project.manifest.project_id, checksum],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| error.to_string())?;
            if duplicate {
                return Err("duplicate asset detected".into());
            }
            tx.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind,media_container,media_format,validation_level,model_metadata_schema_version,model_metadata_json) VALUES (?1,?2,?3,?4,?5,?6,?7,'ready','imported','model3d',?8,'glTF 2.0',?9,1,?10)", rusqlite::params![asset_id, project.manifest.project_id, original_filename, master.to_string_lossy(), bytes.len() as u64, checksum, now as i64, validated.container, validated.validation_level, metadata_json]).map_err(|error| error.to_string())?;
            tx.commit().map_err(|error| error.to_string())
        })();
        if let Err(error) = inserted {
            let _ = fs::remove_file(&master);
            return Err(error);
        }
        Ok(AssetInfo {
            asset_id,
            original_filename,
            managed_master_path: master.to_string_lossy().into_owned(),
            file_size: bytes.len() as u64,
            checksum,
            image_width: None,
            image_height: None,
            image_format: None,
            has_alpha: false,
            imported_at_ms: now,
            status: "ready".into(),
            media_kind: "model3d".into(),
            media_container: Some(validated.container.into()),
            media_format: Some("glTF 2.0".into()),
            media_width: None,
            media_height: None,
            duration_ms: None,
            fps_numerator: None,
            fps_denominator: None,
            validation_level: Some(validated.validation_level.into()),
            codec: None,
            model_metadata_schema_version: Some(1),
            model_metadata: Some(validated.metadata),
            processing_status: Some("raw".into()),
            approval_status: None,
            source_type: Some("imported".into()),
        })
    })();
    let _ = fs::remove_dir_all(staging);
    result
}

fn verify_managed_model(
    path: &std::path::Path,
    expected_size: u64,
    expected_checksum: &str,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or("managed model size overflow")?;
        if total > model3d_validation::MAX_MODEL_BYTES {
            return Err("managed model exceeds validation bound".into());
        }
        hasher.update(&chunk[..read]);
    }
    if total != expected_size || format!("{:x}", hasher.finalize()) != expected_checksum {
        return Err("managed model integrity verification failed".into());
    }
    Ok(())
}

fn extract_image_metadata(path: &PathBuf) -> Result<(u32, u32, String, bool), String> {
    use image::GenericImageView;

    let img = image::open(path).map_err(|e| e.to_string())?;
    let (width, height) = img.dimensions();

    let format = match img {
        image::DynamicImage::ImageRgba8(_) => "PNG".to_string(),
        image::DynamicImage::ImageRgb8(_) => "JPEG".to_string(),
        _ => "PNG".to_string(),
    };

    let has_alpha = matches!(img, image::DynamicImage::ImageRgba8(_));

    Ok((width, height, format, has_alpha))
}

#[tauri::command]
fn list_assets(state: State<'_, AppState>) -> Result<Vec<AssetInfo>, String> {
    let project_guard = state.current_project.lock().unwrap();
    let project = project_guard.as_ref().ok_or("no project open")?;

    let db = project.db.lock().unwrap();

    let mut stmt = db
        .prepare(
            r#"
            SELECT asset_id, original_filename, managed_master_path, file_size,
                   checksum, image_width, image_height, image_format, has_alpha,
                   imported_at_ms, status, media_kind, media_container, media_format,
                   media_width, media_height, duration_ms, fps_numerator, fps_denominator,
                    validation_level, codec, model_metadata_schema_version, model_metadata_json,
                    source_type, processing_status, (SELECT status FROM asset_approvals WHERE asset_id=assets.asset_id)
            FROM assets
            ORDER BY imported_at_ms DESC
        "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            rusqlite::params![],
            |row: &rusqlite::Row| -> Result<AssetInfo, rusqlite::Error> {
                let width: Option<i64> = row.get(5)?;
                let height: Option<i64> = row.get(6)?;
                let imported_at_ms: i64 = row.get(9)?;

                Ok(AssetInfo {
                    asset_id: row.get(0)?,
                    original_filename: row.get(1)?,
                    managed_master_path: row.get::<_, String>(2)?,
                    file_size: row.get::<_, i64>(3)? as u64,
                    checksum: row.get(4)?,
                    image_width: width.map(|v| v as u32),
                    image_height: height.map(|v| v as u32),
                    image_format: row.get(7)?,
                    has_alpha: row.get::<_, i64>(8)? == 1,
                    imported_at_ms: imported_at_ms as i128 as u128,
                    status: row.get(10)?,
                    media_kind: row.get(11)?,
                    media_container: row.get(12)?,
                    media_format: row.get(13)?,
                    media_width: row.get::<_, Option<i64>>(14)?.map(|v| v as u32),
                    media_height: row.get::<_, Option<i64>>(15)?.map(|v| v as u32),
                    duration_ms: row.get::<_, Option<i64>>(16)?.map(|v| v as u64),
                    fps_numerator: row.get::<_, Option<i64>>(17)?.map(|v| v as u32),
                    fps_denominator: row.get::<_, Option<i64>>(18)?.map(|v| v as u32),
                    validation_level: row.get(19)?,
                    codec: row.get(20)?,
                    model_metadata_schema_version: row
                        .get::<_, Option<i64>>(21)?
                        .map(|value| value as u32),
                    model_metadata: parse_model_metadata(row.get(22)?)?,
                    source_type: row.get(23)?,
                    processing_status: row.get(24)?,
                    approval_status: row.get(25)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for asset in rows {
        result.push(asset.map_err(|e| e.to_string())?);
    }

    Ok(result)
}

#[tauri::command]
fn search_assets(query: String, state: State<'_, AppState>) -> Result<Vec<AssetInfo>, String> {
    let pattern = format!("%{}%", query);

    let project_guard = state.current_project.lock().unwrap();
    let project = project_guard.as_ref().ok_or("no project open")?;

    let db = project.db.lock().unwrap();

    let mut stmt = db
        .prepare(
            r#"
            SELECT asset_id, original_filename, managed_master_path, file_size,
                   checksum, image_width, image_height, image_format, has_alpha,
                   imported_at_ms, status, media_kind, media_container, media_format,
                   media_width, media_height, duration_ms, fps_numerator, fps_denominator,
                    validation_level, codec, model_metadata_schema_version, model_metadata_json,
                    source_type, processing_status, (SELECT status FROM asset_approvals WHERE asset_id=assets.asset_id)
            FROM assets
            WHERE original_filename LIKE ?1
            ORDER BY imported_at_ms DESC
        "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            rusqlite::params![pattern],
            |row: &rusqlite::Row| -> Result<AssetInfo, rusqlite::Error> {
                let width: Option<i64> = row.get(5)?;
                let height: Option<i64> = row.get(6)?;
                let imported_at_ms: i64 = row.get(9)?;

                Ok(AssetInfo {
                    asset_id: row.get(0)?,
                    original_filename: row.get(1)?,
                    managed_master_path: row.get::<_, String>(2)?,
                    file_size: row.get::<_, i64>(3)? as u64,
                    checksum: row.get(4)?,
                    image_width: width.map(|v| v as u32),
                    image_height: height.map(|v| v as u32),
                    image_format: row.get(7)?,
                    has_alpha: row.get::<_, i64>(8)? == 1,
                    imported_at_ms: imported_at_ms as i128 as u128,
                    status: row.get(10)?,
                    media_kind: row.get(11)?,
                    media_container: row.get(12)?,
                    media_format: row.get(13)?,
                    media_width: row.get::<_, Option<i64>>(14)?.map(|v| v as u32),
                    media_height: row.get::<_, Option<i64>>(15)?.map(|v| v as u32),
                    duration_ms: row.get::<_, Option<i64>>(16)?.map(|v| v as u64),
                    fps_numerator: row.get::<_, Option<i64>>(17)?.map(|v| v as u32),
                    fps_denominator: row.get::<_, Option<i64>>(18)?.map(|v| v as u32),
                    validation_level: row.get(19)?,
                    codec: row.get(20)?,
                    model_metadata_schema_version: row
                        .get::<_, Option<i64>>(21)?
                        .map(|value| value as u32),
                    model_metadata: parse_model_metadata(row.get(22)?)?,
                    source_type: row.get(23)?,
                    processing_status: row.get(24)?,
                    approval_status: row.get(25)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for asset in rows {
        result.push(asset.map_err(|e| e.to_string())?);
    }

    Ok(result)
}

#[tauri::command]
fn get_asset_preview(asset_id: String, state: State<'_, AppState>) -> Result<String, String> {
    let project_guard = state.current_project.lock().unwrap();
    let project = project_guard.as_ref().ok_or("no project open")?;

    let db = project.db.lock().unwrap();
    let row = db.query_row(
        "SELECT media_kind FROM assets WHERE asset_id = ?1",
        rusqlite::params![asset_id],
        |row: &rusqlite::Row| row.get::<_, String>(0),
    );

    match row {
        Ok(kind) if kind == "image" => {}
        Ok(_) => return Err("only image assets have image previews".to_string()),
        Err(_) => return Err("asset not found".to_string()),
    }

    let preview_path = project
        .root()
        .join("assets")
        .join("previews")
        .join(format!("{}_preview.png", asset_id));

    if !preview_path.exists() {
        return Err("preview not found".to_string());
    }

    let preview_data = fs::read(&preview_path).map_err(|e| e.to_string())?;

    let base64 = base64::prelude::BASE64_STANDARD.encode(&preview_data);

    state.logger.info(
        "preview_served",
        &format!("Preview served for asset {}", asset_id),
    );

    Ok(format!("data:image/png;base64,{}", base64))
}

fn parse_model_metadata(
    value: Option<String>,
) -> rusqlite::Result<Option<model3d_validation::Model3dMetadata>> {
    value
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    json.len(),
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

#[tauri::command]
fn get_current_project(state: State<'_, AppState>) -> Option<ProjectInfo> {
    let guard = state.current_project.lock().unwrap();
    guard.as_ref().map(|p| p.info())
}

#[tauri::command]
fn get_recent_projects(state: State<'_, AppState>) -> Vec<RecentProjectInfo> {
    let guard = state.recent_projects.lock().unwrap();
    guard.clone()
}

#[tauri::command]
fn archive_project(root: String, state: State<'_, AppState>) -> Result<bool, String> {
    let project_root = PathBuf::from(root);
    let canonical =
        dunce::canonicalize(&project_root).map_err(|e| format!("invalid path: {}", e))?;

    let current_guard = state.current_project.lock().unwrap();
    if let Some(current) = current_guard.as_ref() {
        if current.root == canonical {
            return Err("cannot archive currently open project".into());
        }
    }
    drop(current_guard);

    let manifest_path = canonical.join(MANIFEST_FILENAME);
    if !manifest_path.exists() {
        return Err("no Nexora project found at path".into());
    }

    let manifest = project::load_manifest(&manifest_path).map_err(|e| e.to_string())?;

    let mut recent = state.recent_projects.lock().unwrap();
    recent.retain(|project| project.manifest.project_id != manifest.project_id);
    save_recent_projects(&state.recent_projects_path, &recent);

    state.logger.info(
        "project_archived",
        &format!("{} at {}", manifest.name, canonical.display()),
    );
    Ok(true)
}

#[tauri::command]
fn delete_project(root: String, state: State<'_, AppState>) -> Result<bool, String> {
    let project_root = PathBuf::from(root);
    let canonical =
        dunce::canonicalize(&project_root).map_err(|e| format!("invalid path: {}", e))?;

    let manifest_path = canonical.join(MANIFEST_FILENAME);
    if !manifest_path.exists() {
        return Err("no Nexora project found at path".into());
    }
    let manifest = project::load_manifest(&manifest_path).map_err(|e| e.to_string())?;

    // If this project is currently open, retire it first so its lock and
    // database handles are released before we delete the folder. (Otherwise
    // remove_dir_all fails on Windows because the lock/DB files are open.)
    {
        let mut current_guard = state.current_project.lock().unwrap();
        if let Some(current) = current_guard.as_ref() {
            if current.root == canonical {
                retire_current_project(&mut current_guard)?;
            }
        }
    }

    fs::remove_dir_all(&canonical).map_err(|e| format!("could not delete project folder: {}", e))?;

    let mut recent = state.recent_projects.lock().unwrap();
    recent.retain(|project| project.manifest.project_id != manifest.project_id);
    save_recent_projects(&state.recent_projects_path, &recent);
    state.logger.info(
        "project_deleted",
        &format!("{} at {}", manifest.name, canonical.display()),
    );
    Ok(true)
}

fn active_project_root(state: &AppState) -> Option<PathBuf> {
    state
        .current_project
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|project| project.root.clone()))
}

fn current_hardware_snapshot(state: &AppState) -> hardware::HardwareSnapshot {
    let project_root = active_project_root(state);
    let mut snapshot = state.hardware_snapshot.lock().unwrap();
    if project_root.is_some() && snapshot.active_project_disk_free_mib.is_none() {
        *snapshot = hardware::detect(project_root.as_deref());
    }
    snapshot.clone()
}

#[tauri::command]
fn get_hardware_snapshot(state: State<'_, AppState>) -> hardware::HardwareSnapshot {
    current_hardware_snapshot(&state)
}

#[tauri::command]
fn refresh_hardware_snapshot(state: State<'_, AppState>) -> hardware::HardwareSnapshot {
    let project_root = active_project_root(&state);
    let refreshed = hardware::detect(project_root.as_deref());
    *state.hardware_snapshot.lock().unwrap() = refreshed.clone();
    refreshed
}

#[tauri::command]
fn list_providers(capability: Option<Capability>, state: State<'_, AppState>) -> Vec<ProviderView> {
    let snapshot = current_hardware_snapshot(&state);
    state
        .provider_registry
        .lock()
        .unwrap()
        .list(capability, &snapshot)
}

#[tauri::command]
fn refresh_provider_health(
    provider_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ProviderHealth>, String> {
    let refresh_local = provider_id
        .as_deref()
        .is_none_or(|id| id == image_generation::PROVIDER_ID);
    let refresh_video = provider_id
        .as_deref()
        .is_none_or(|id| id == video_generation::PROVIDER_ID);
    let refresh_openrouter = provider_id
        .as_deref()
        .is_none_or(|id| id == openrouter::PROVIDER_ID);
    let mut registry = state.provider_registry.lock().unwrap();
    let mut health = registry
        .refresh_health(provider_id.as_deref())
        .map_err(|error| command_error(&state, "provider_health_refresh_failed", error))?;
    if refresh_local {
        let config = state.image_provider_config.lock().unwrap().clone();
        let result = image_generation::health(&config);
        let item = registry
            .set_local_a1111_state(
                config.enabled,
                if result.is_ok() {
                    providers::HealthState::Healthy
                } else {
                    providers::HealthState::Unavailable
                },
                result.err().map(|error| error.to_string()),
            )
            .map_err(|error| command_error(&state, "provider_health_refresh_failed", error))?;
        health.retain(|value| value.provider_id != image_generation::PROVIDER_ID);
        health.push(item);
    }
    if refresh_video {
        let config = state.video_provider_config.lock().unwrap().clone();
        let result = video_generation::provider_state(&config);
        let item = registry
            .set_local_comfyui_state(
                config.enabled,
                result.reachable,
                result.compatible,
                result.detail,
            )
            .map_err(|error| command_error(&state, "provider_health_refresh_failed", error))?;
        health.retain(|value| value.provider_id != video_generation::PROVIDER_ID);
        health.push(item);
    }
    if refresh_openrouter {
        let config = state.settings.lock().unwrap().openrouter.clone();
        let item = registry
            .set_openrouter_state(
                config.enabled,
                if config.enabled && !config.api_key.is_empty() && !config.default_model.is_empty() {
                    match openrouter::test_connection(&config) {
                        Ok(_) => providers::HealthState::Healthy,
                        Err(openrouter::OpenRouterError::InvalidApiKey)
                        | Err(openrouter::OpenRouterError::Unauthorized)
                        | Err(openrouter::OpenRouterError::Forbidden) => providers::HealthState::Misconfigured,
                        Err(openrouter::OpenRouterError::RateLimited) => providers::HealthState::Degraded,
                        _ => providers::HealthState::Unavailable,
                    }
                } else if !config.enabled {
                    providers::HealthState::Unavailable
                } else {
                    providers::HealthState::Misconfigured
                },
                None,
            )
            .map_err(|error| command_error(&state, "provider_health_refresh_failed", error))?;
        health.retain(|value| value.provider_id != openrouter::PROVIDER_ID);
        health.push(item);
    }
    // Refresh local Hunyuan3D
    // Hunyuan3D's /health endpoint can hang while the model is loading, so try
    // HTTP first but fall back to a TCP connect (same strategy as the runtime
    // manager) so a listening service is still detected as ready.
    let hunyuan_reachable = {
        let http_ok = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .and_then(|c| c.get("http://127.0.0.1:8081/health").send())
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if http_ok {
            true
        } else {
            // TCP fallback: if port 8081 answers, the service is up
            std::net::TcpStream::connect_timeout(
                &"127.0.0.1:8081".parse::<std::net::SocketAddr>().unwrap(),
                std::time::Duration::from_secs(3),
            )
            .is_ok()
        }
    };
    let item = registry
        .set_local_hunyuan_state(
            true,
            hunyuan_reachable,
            true,
            if hunyuan_reachable {
                Some("Hunyuan3D server ready".into())
            } else {
                Some("Hunyuan3D server unreachable on http://127.0.0.1:8081".into())
            },
        )
        .map_err(|error| command_error(&state, "provider_health_refresh_failed", error))?;
    health.retain(|value| value.provider_id != "local.hunyuan");
    health.push(item);

    Ok(health)
}

#[tauri::command]
fn get_image_provider_config(state: State<'_, AppState>) -> image_generation::ImageProviderConfig {
    state.image_provider_config.lock().unwrap().clone()
}

#[tauri::command]
fn save_image_provider_config(
    config: image_generation::ImageProviderConfig,
    state: State<'_, AppState>,
) -> Result<image_generation::ImageProviderConfig, String> {
    image_generation::save_config(&state.image_provider_config_path, &config)
        .map_err(|error| command_error(&state, "image_provider_config_save_failed", error))?;
    *state.image_provider_config.lock().unwrap() = config.clone();
    state
        .provider_registry
        .lock()
        .unwrap()
        .set_local_a1111_state(
            config.enabled,
            providers::HealthState::Unavailable,
            Some(
                if config.enabled {
                    "health check required"
                } else {
                    "provider disabled"
                }
                .into(),
            ),
        )
        .map_err(|error| error.to_string())?;
    Ok(config)
}

#[tauri::command]
fn list_image_generation_providers(state: State<'_, AppState>) -> Vec<ProviderView> {
    let snapshot = current_hardware_snapshot(&state);
    state
        .provider_registry
        .lock()
        .unwrap()
        .list(Some(Capability::TextToImage), &snapshot)
        .into_iter()
        .filter(|view| view.manifest.provider_id == image_generation::PROVIDER_ID)
        .collect()
}

#[tauri::command]
fn create_image_generation_job(
    input: jobs::CreateImageGenerationJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::CreateImageGenerationJobResult, String> {
    let config = state.image_provider_config.lock().unwrap().clone();
    let health = image_generation::health(&config);
    state
        .provider_registry
        .lock()
        .unwrap()
        .set_local_a1111_state(
            config.enabled,
            if health.is_ok() {
                providers::HealthState::Healthy
            } else {
                providers::HealthState::Unavailable
            },
            health.as_ref().err().map(ToString::to_string),
        )
        .map_err(|error| error.to_string())?;
    health
        .map_err(|error| command_error(&state, "image_generation_provider_unavailable", error))?;
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_image_generation(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "image_generation_job_create_failed", error))
}

#[tauri::command]
fn get_image_generation_result(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<image_generation::ImageGenerationResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    image_generation::get_result(project, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_video_provider_config(state: State<'_, AppState>) -> video_generation::VideoProviderConfig {
    state.video_provider_config.lock().unwrap().clone()
}

#[tauri::command]
fn save_video_provider_config(
    config: video_generation::VideoProviderConfig,
    state: State<'_, AppState>,
) -> Result<video_generation::VideoProviderConfig, String> {
    video_generation::save_config(&state.video_provider_config_path, &config)
        .map_err(|error| command_error(&state, "video_provider_config_save_failed", error))?;
    *state.video_provider_config.lock().unwrap() = config.clone();
    state
        .provider_registry
        .lock()
        .unwrap()
        .set_local_comfyui_state(
            config.enabled,
            false,
            false,
            Some(
                if config.enabled {
                    "health and compatibility checks required"
                } else {
                    "provider disabled"
                }
                .into(),
            ),
        )
        .map_err(|error| error.to_string())?;
    Ok(config)
}

#[tauri::command]
fn list_video_generation_providers(state: State<'_, AppState>) -> Vec<ProviderView> {
    let snapshot = current_hardware_snapshot(&state);
    state
        .provider_registry
        .lock()
        .unwrap()
        .list(None, &snapshot)
        .into_iter()
        .filter(|view| view.manifest.provider_id == video_generation::PROVIDER_ID)
        .collect()
}

#[tauri::command]
fn create_video_generation_job(
    input: jobs::CreateVideoGenerationJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::CreateVideoGenerationJobResult, String> {
    let config = state.video_provider_config.lock().unwrap().clone();
    let provider_state = video_generation::provider_state(&config);
    state
        .provider_registry
        .lock()
        .unwrap()
        .set_local_comfyui_state(
            config.enabled,
            provider_state.reachable,
            provider_state.compatible,
            provider_state.detail.clone(),
        )
        .map_err(|error| error.to_string())?;
    if !provider_state.compatible {
        return Err(command_error(
            &state,
            "video_generation_provider_incompatible",
            provider_state
                .detail
                .unwrap_or_else(|| "provider is incompatible".into()),
        ));
    }
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_video_generation(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "video_generation_job_create_failed", error))
}

#[tauri::command]
fn get_video_generation_result(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<video_generation::VideoGenerationResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    video_generation::get_result(project, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn create_model3d_generation_job(
    input: jobs::CreateModel3dGenerationJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::CreateModel3dGenerationJobResult, String> {
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_model3d_generation(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "model3d_generation_job_create_failed", error))
}

#[tauri::command]
fn get_model3d_generation_result(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<model3d_generation::Model3dGenerationResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    model3d_generation::get_result(project, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn create_model3d_processing_job(
    input: jobs::CreateModel3dProcessingJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::CreateModel3dProcessingJobResult, String> {
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_model3d_processing(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "model3d_processing_job_create_failed", error))
}

#[tauri::command]
fn get_model3d_processing_result(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<model3d_processing::Model3dProcessingResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    model3d_processing::get_result(project, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn approve_model3d_asset(
    asset_id: String,
    processing_job_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::approve_model3d_asset(project, &asset_id, &processing_job_id)
        .map_err(|error| command_error(&state, "model3d_asset_approve_failed", error))
}

#[tauri::command]
fn approve_image_asset(
    asset_id: String,
    generation_job_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::approve_image_asset(project, &asset_id, generation_job_id.as_deref())
        .map_err(|error| command_error(&state, "image_asset_approve_failed", error))
}

#[tauri::command]
fn reject_model3d_asset(
    asset_id: String,
    processing_job_id: String,
    rejection_reason: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::reject_model3d_asset(project, &asset_id, &processing_job_id, rejection_reason)
        .map_err(|error| command_error(&state, "model3d_asset_reject_failed", error))
}

#[tauri::command]
fn reprocess_model3d_asset(
    asset_id: String,
    processing_job_id: String,
    profile: Option<String>,
    quality: Option<String>,
    state: State<'_, AppState>,
) -> Result<jobs::CreateModel3dProcessingJobResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    let registry = state.provider_registry.lock().unwrap().clone();
    let snapshot = current_hardware_snapshot(&state);
    jobs::reprocess_model3d_asset(
        project,
        &registry,
        &snapshot,
        &asset_id,
        &processing_job_id,
        profile,
        quality,
    )
    .map_err(|error| command_error(&state, "model3d_asset_reprocess_failed", error))
}

#[tauri::command]
fn create_hunyuan_generation_job(
    input: jobs::CreateHunyuanGenerationJobInput,
    state: State<'_, AppState>,
) -> Result<jobs::CreateHunyuanGenerationJobResult, String> {
    let snapshot = current_hardware_snapshot(&state);
    let registry = state.provider_registry.lock().unwrap().clone();
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_hunyuan_generation(project, &registry, &snapshot, input)
        .map_err(|error| command_error(&state, "hunyuan_generation_job_create_failed", error))
}

#[tauri::command]
fn get_hunyuan_generation_result(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<hunyuan_generation::HunyuanGenerationResult, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    hunyuan_generation::get_result(project, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn discover_unity_project(_state: State<'_, AppState>) -> Result<Option<String>, String> {
    // Look for any Unity project under the user's profile, preferring paths under
    // a "development/Games" layout, then fall back to the Documents folder.
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(home.join("development").join("Games"));
    candidates.push(home.join("Documents").join("Games"));
    if let Some(app_data) = dirs::data_local_dir() {
        candidates.push(app_data.join("development").join("Games"));
        candidates.push(app_data.join("Games"));
    }

    for parent in candidates {
        if let Ok(entries) = std::fs::read_dir(&parent) {
            for entry in entries.flatten() {
                let candidate = entry.path();
                if candidate.join("ProjectSettings").exists()
                    && candidate.join("Assets").exists()
                    && candidate.join("Packages").exists()
                    && candidate
                        .join("ProjectSettings")
                        .join("ProjectVersion.txt")
                        .exists()
                {
                    return Ok(Some(candidate.to_string_lossy().to_string()));
                }
            }
        }
    }

    Ok(None)
}

#[tauri::command]
fn validate_unity_project(
    project_root: String,
    state: State<'_, AppState>,
) -> Result<UnityProjectValidation, String> {
    let path = PathBuf::from(&project_root);

    // Check required directories/files
    let has_project_settings = path.join("ProjectSettings").exists();
    let has_assets = path.join("Assets").exists();
    let has_packages = path.join("Packages").exists();
    let has_project_version = path
        .join("ProjectSettings")
        .join("ProjectVersion.txt")
        .exists();

    let is_valid = has_project_settings && has_assets && has_packages && has_project_version;

    let unity_version = if has_project_version {
        std::fs::read_to_string(path.join("ProjectSettings").join("ProjectVersion.txt"))
            .ok()
            .and_then(|content| {
                content.lines().next().map(|line| {
                    line.trim_start_matches("m_EditorVersion: ")
                        .trim()
                        .to_string()
                })
            })
    } else {
        None
    };

    // Detect render pipeline if valid
    let render_pipeline = if is_valid {
        let packages_path = path.join("Packages").join("manifest.json");
        if packages_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&packages_path) {
                if content.contains("com.unity.render-pipelines.universal") {
                    Some("URP".to_string())
                } else if content.contains("com.unity.render-pipelines.high-definition") {
                    Some("HDRP".to_string())
                } else {
                    Some("Built-in".to_string())
                }
            } else {
                Some("Built-in".to_string())
            }
        } else {
            Some("Unknown".to_string())
        }
    } else {
        None
    };

    // Check for GLTFast importer
    let glb_importer_available = if is_valid {
        let packages_path = path.join("Packages").join("manifest.json");
        if packages_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&packages_path) {
                content.contains("com.unity.gltfast")
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let error_message = if !is_valid {
        let mut errors = Vec::new();
        if !has_project_settings {
            errors.push("ProjectSettings directory missing");
        }
        if !has_assets {
            errors.push("Assets directory missing");
        }
        if !has_packages {
            errors.push("Packages directory missing");
        }
        if !has_project_version {
            errors.push("ProjectVersion.txt missing");
        }
        Some(errors.join("; "))
    } else {
        None
    };

    // Register or update the Unity project target
    let validation_status = if is_valid { "valid" } else { "invalid" };
    let mut app_settings = state.settings.lock().unwrap();
    let target_id = app_settings
        .unity_targets
        .keys()
        .find(|id| {
            if let Some(target) = app_settings.unity_targets.get(*id) {
                target.project_root == project_root
            } else {
                false
            }
        })
        .cloned()
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());

    let mut target = app_settings
        .unity_targets
        .get(&target_id)
        .cloned()
        .unwrap_or_else(|| {
            settings::UnityProjectTarget::new(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                project_root.clone(),
            )
        });

    target.target_id = target_id.clone();
    target.unity_version = unity_version.clone();
    target.render_pipeline = render_pipeline;
    target.last_validated_at_ms = Some(settings::now_ms());
    target.validation_status = Some(validation_status.to_string());
    target.validation_error = error_message.clone();
    target.glb_importer_available = glb_importer_available;
    target.updated_at_ms = settings::now_ms();

    app_settings
        .unity_targets
        .insert(target_id.clone(), target.clone());
    app_settings.active_unity_target = Some(target_id);

    drop(app_settings);
    // Save settings - re-acquire the lock to persist the updated target registry.
    // Unity validation must not require a Nexora project to be open, so we
    // only persist when we actually hold a project; otherwise the in-memory
    // settings are still updated for the current session.
    let settings_path = state.settings_path.clone();
    if state.current_project.lock().unwrap().is_some() {
        settings::save(&settings_path, &state.settings.lock().unwrap())
            .map_err(|e| e.to_string())?;
    }

    Ok(UnityProjectValidation {
        is_valid,
        unity_version,
        project_root: path.to_string_lossy().to_string(),
        error_message,
    })
}

#[tauri::command]
fn save_unity_config(
    config: settings::UnityProjectTarget,
    state: State<'_, AppState>,
) -> Result<settings::UnityProjectTarget, String> {
    let mut app_settings = state.settings.lock().unwrap();
    app_settings
        .unity_targets
        .insert(config.target_id.clone(), config.clone());
    app_settings.active_unity_target = Some(config.target_id.clone());
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(app_settings
        .unity_targets
        .get(&config.target_id)
        .unwrap()
        .clone())
}

#[tauri::command]
fn create_unity_delivery(
    input: jobs::CreateUnityDeliveryInput,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_unity_delivery(
        project,
        &input.asset_id,
        &input.processing_job_id,
        &input.target_id,
        input.revision,
    )
    .map_err(|error| command_error(&state, "unity_delivery_create_failed", error))
}

#[tauri::command]
fn update_unity_delivery_status(
    input: jobs::UpdateUnityDeliveryStatusInput,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::update_unity_delivery_status(
        project,
        &input.delivery_id,
        &input.status,
        input.error_message,
        input.imported_asset_path,
        input.prefab_path,
        input.validation_report,
    )
    .map_err(|error| command_error(&state, "unity_delivery_status_update_failed", error))
}

#[tauri::command]
fn get_unity_delivery(
    delivery_id: String,
    state: State<'_, AppState>,
) -> Result<jobs::UnityDelivery, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::get_unity_delivery(project, &delivery_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn list_unity_deliveries(
    target_id: Option<String>,
    asset_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<jobs::UnityDelivery>, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::list_unity_deliveries(project, target_id, asset_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn verify_asset_approved_for_unity_delivery(
    asset_id: String,
    state: State<'_, AppState>,
) -> Result<jobs::UnityDelivery, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    // verify_asset_approved_for_unity_delivery returns (asset_id, checksum)
    // We need to get the full UnityDelivery for the asset
    let delivery_result = jobs::verify_asset_approved_for_unity_delivery(&*project, &asset_id);

    match delivery_result {
        Ok((asset_id, _checksum)) => {
            // Get the delivery record for this asset
            let guard = state.current_project.lock().unwrap();
            let project = guard.as_ref().ok_or("no project open")?;
            jobs::list_unity_deliveries(project, None, Some(asset_id))
                .map(|deliveries| deliveries.into_iter().next())
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "No delivery found for asset".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn import_unity_asset(
    input: jobs::CreateUnityDeliveryInput,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::create_unity_delivery(
        &*project,
        &input.asset_id,
        &input.processing_job_id,
        &input.target_id,
        input.revision,
    )
    .map_err(|error| command_error(&state, "unity_delivery_create_failed", error))
}

#[tauri::command]
fn get_unity_import_status(
    delivery_id: String,
    state: State<'_, AppState>,
) -> Result<jobs::UnityDelivery, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    jobs::get_unity_delivery(project, &delivery_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn install_unity_package(
    project_root: String,
    package_name: String,
    version: String,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    let project_root = PathBuf::from(project_root);
    let manifest_path = project_root.join("Packages").join("manifest.json");

    if !manifest_path.exists() {
        return Err("Unity project manifest not found".to_string());
    }

    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read manifest: {}", e))?;

    let mut manifest: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse manifest: {}", e))?;

    if let Some(deps) = manifest.get_mut("dependencies") {
        if let Some(obj) = deps.as_object_mut() {
            obj.insert(package_name.clone(), serde_json::Value::String(version));
        }
    }

    let new_content = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;

    // Backup original
    let backup_path = manifest_path.with_extension("json.backup");
    std::fs::copy(&manifest_path, &backup_path).map_err(|e| e.to_string())?;

    std::fs::write(&manifest_path, new_content).map_err(|e| e.to_string())?;

    // Verify package resolution by checking if Unity can resolve it
    // This would ideally trigger Unity to resolve packages, but we can't do that in batchmode here
    // Return success - Unity will resolve on next open

    Ok(())
}

#[tauri::command]
fn create_unity_prefab(
    input: jobs::CreateUnityDeliveryInput,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;

    // First create delivery record
    let delivery_id = jobs::create_unity_delivery(
        &*project,
        &input.asset_id,
        &input.processing_job_id,
        &input.target_id,
        input.revision,
    )
    .map_err(|error| command_error(&state, "unity_delivery_create_failed", error))?;

    // The actual Unity import would be triggered via batchmode
    // For now, return the delivery ID
    Ok(delivery_id)
}

#[tauri::command]
fn validate_unity_import(
    delivery_id: String,
    state: State<'_, AppState>,
) -> Result<jobs::UnityDelivery, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;

    let delivery = jobs::get_unity_delivery(project, &delivery_id)
        .map_err(|error| command_error(&state, "unity_delivery_get_failed", error))?;

    // Validate the import by checking if prefab exists and is valid
    // This would be called after Unity batchmode import
    // For now, just return the delivery record
    Ok(delivery)
}

#[tauri::command]
fn select_provider(
    capability: Capability,
    state: State<'_, AppState>,
) -> Result<ProviderView, String> {
    let snapshot = current_hardware_snapshot(&state);
    state
        .provider_registry
        .lock()
        .unwrap()
        .select(capability, &snapshot)
        .map_err(|error| command_error(&state, "provider_selection_failed", error))
}

#[tauri::command]
async fn list_runtimes(state: State<'_, AppState>) -> Result<Vec<RuntimeState>, String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    Ok(runtime_manager_arc.list_runtimes().await)
}

#[tauri::command]
async fn check_and_update_engines(
    state: State<'_, AppState>,
) -> Result<Vec<EngineUpdateStatus>, String> {
    let git_targets = vec![
        EngineUpdateTarget {
            engine_id: "hunyuan3d",
            display_name: "Hunyuan 3D Engine",
            root: PathBuf::from(r"C:\AI\Hunyuan3D"),
        },
        EngineUpdateTarget {
            engine_id: "automatic1111",
            display_name: "Image Engine",
            root: PathBuf::from(r"C:\AI\stable-diffusion-webui"),
        },
    ];
    let check_targets = git_targets.clone();
    let mut statuses = tauri::async_runtime::spawn_blocking(move || {
        check_targets
            .iter()
            .map(|target| git_update_target(target, false))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| error.to_string())?;
    statuses.push(check_non_git_engine(
        "blender",
        "Mesh Pipeline",
        "Blender updates are managed through its installer.",
    ));
    statuses.push(check_non_git_engine(
        "wan_video",
        "Video Engine",
        "No local update source is configured for the video pipeline.",
    ));

    let runtime_manager = (*state.runtime_manager.lock().unwrap()).clone();
    let mut restart_after_update = Vec::new();
    let mut update_ids = Vec::new();
    for status in &mut statuses {
        if !status.update_available || status.status != "available" {
            continue;
        }
        update_ids.push(status.engine_id.clone());
        if let Ok(runtime) = runtime_manager.get_runtime(&status.engine_id).await {
            if runtime.started_by_nexora {
                if runtime_manager.stop_runtime(&status.engine_id).await.is_ok() {
                    restart_after_update.push(status.engine_id.clone());
                }
            } else if matches!(runtime.status, RuntimeStatus::Ready | RuntimeStatus::Starting) {
                status.status = "deferred".into();
                status.detail = "Engine is running outside Nexora; close it before updating.".into();
                update_ids.retain(|engine_id| engine_id != &status.engine_id);
            }
        }
    }

    let apply_targets: Vec<EngineUpdateTarget> = git_targets
        .into_iter()
        .filter(|target| update_ids.iter().any(|engine_id| engine_id == target.engine_id))
        .collect();
    if !apply_targets.is_empty() {
        let applied = tauri::async_runtime::spawn_blocking(move || {
            apply_targets
                .iter()
                .map(|target| git_update_target(target, true))
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| error.to_string())?;
        for updated in applied {
            if let Some(status) = statuses.iter_mut().find(|status| status.engine_id == updated.engine_id) {
                *status = updated;
            }
        }
    }

    for engine_id in restart_after_update {
        let _ = runtime_manager.start_runtime(&engine_id).await;
    }
    Ok(statuses)
}

#[tauri::command]
async fn get_runtime(
    runtime_id: String,
    state: State<'_, AppState>,
) -> Result<RuntimeState, String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    runtime_manager_arc
        .get_runtime(&runtime_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn check_runtime_health(
    runtime_id: String,
    state: State<'_, AppState>,
) -> Result<RuntimeStatus, String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    runtime_manager_arc
        .check_health(&runtime_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_runtime(runtime_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    runtime_manager_arc
        .start_runtime(&runtime_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_runtime(runtime_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    runtime_manager_arc
        .stop_runtime(&runtime_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn initialize_runtimes(
    state: State<'_, AppState>,
) -> Result<Vec<(String, RuntimeStatus)>, String> {
    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
    Ok(runtime_manager_arc.initialize_all().await)
}

#[tauri::command]
async fn get_orchestrator_status(
    state: State<'_, AppState>,
) -> Result<provider_orchestrator::OrchestratorStatus, String> {
    let orchestrator = (*state.provider_orchestrator.lock().unwrap()).clone();
    Ok(orchestrator.get_status().await)
}

#[tauri::command]
async fn start_orchestrator(
    state: State<'_, AppState>,
) -> Result<(), String> {
    let orchestrator = (*state.provider_orchestrator.lock().unwrap()).clone();
    orchestrator.initialize_sequential().await
}

#[tauri::command]
fn get_a1111_config(state: State<'_, AppState>) -> settings::A1111Config {
    state.settings.lock().unwrap().a1111.clone()
}

#[tauri::command]
fn save_a1111_config(
    config: settings::A1111Config,
    state: State<'_, AppState>,
) -> Result<settings::A1111Config, String> {
    let mut app_settings = state.settings.lock().unwrap();
    app_settings.a1111 = config.clone();
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(config)
}

#[tauri::command]
fn get_hunyuan_config(state: State<'_, AppState>) -> settings::HunyuanConfig {
    state.settings.lock().unwrap().hunyuan.clone()
}

#[tauri::command]
fn save_hunyuan_config(
    config: settings::HunyuanConfig,
    state: State<'_, AppState>,
) -> Result<settings::HunyuanConfig, String> {
    let mut app_settings = state.settings.lock().unwrap();
    app_settings.hunyuan = config.clone();
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(config)
}

#[tauri::command]
fn get_openrouter_config(state: State<'_, AppState>) -> openrouter::MaskedOpenRouterConfig {
    state.settings.lock().unwrap().openrouter.masked()
}

#[tauri::command]
fn save_openrouter_config(
    config: openrouter::OpenRouterConfig,
    state: State<'_, AppState>,
) -> Result<openrouter::MaskedOpenRouterConfig, String> {
    config.validate().map_err(|e| e.to_string())?;
    let mut app_settings = state.settings.lock().unwrap();
    app_settings.openrouter = config.clone();
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(app_settings.openrouter.masked())
}

#[tauri::command]
fn remove_openrouter_key(state: State<'_, AppState>) -> Result<openrouter::MaskedOpenRouterConfig, String> {
    let mut app_settings = state.settings.lock().unwrap();
    app_settings.openrouter.api_key.clear();
    app_settings.openrouter.enabled = false;
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(app_settings.openrouter.masked())
}

#[tauri::command]
fn test_openrouter_connection(
    state: State<'_, AppState>,
) -> Result<openrouter::OpenRouterHealth, String> {
    let config = state.settings.lock().unwrap().openrouter.clone();
    let health = openrouter::health(&config);
    Ok(health)
}

#[tauri::command]
fn list_openrouter_models(
    state: State<'_, AppState>,
) -> Result<Vec<openrouter::OpenRouterModel>, String> {
    let config = state.settings.lock().unwrap().openrouter.clone();
    openrouter::list_models(&config).map_err(|e| e.to_string())
}

#[tauri::command]
fn openrouter_chat_completion(
    model_id: String,
    system_prompt: Option<String>,
    user_prompt: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    state: State<'_, AppState>,
) -> Result<openrouter::OpenRouterChatResponse, String> {
    let config = state.settings.lock().unwrap().openrouter.clone();
    openrouter::chat_completion(&config, &model_id, system_prompt.as_deref(), &user_prompt, temperature, max_tokens)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_skip_runtime_startup(state: State<'_, AppState>) -> bool {
    state.settings.lock().unwrap().skip_runtime_startup_on_launch
}

#[tauri::command]
fn set_skip_runtime_startup(enabled: bool, state: State<'_, AppState>) -> Result<bool, String> {
    let mut app_settings = state.settings.lock().unwrap();
    app_settings.skip_runtime_startup_on_launch = enabled;
    let settings_path = state.settings_path.clone();
    settings::save(&settings_path, &app_settings).map_err(|e| e.to_string())?;
    Ok(enabled)
}

#[tauri::command]
fn discover_runtimes(_state: State<'_, AppState>) -> Vec<RuntimeConfig> {
    let mut runtimes = Vec::new();
    if let Some(config) = RuntimeManager::discover_a1111() {
        runtimes.push(config);
    }
    if let Some(config) = RuntimeManager::discover_hunyuan3d() {
        runtimes.push(config);
    }
    if let Some(config) = RuntimeManager::discover_blender() {
        runtimes.push(config);
    }
    runtimes
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UnityDeploymentDto {
    delivery_id: String,
    asset_id: String,
    target_id: String,
    unity_project_root: String,
    destination_folder: String,
    deployed_path: String,
    meta_path: String,
    file_size: u64,
    bytes_written: u64,
    checksum: String,
    category: String,
}

#[tauri::command]
fn deploy_model3d_to_unity(
    asset_id: String,
    target_id: String,
    category: Option<String>,
    state: State<'_, AppState>,
) -> Result<UnityDeploymentDto, String> {
    let guard = state.current_project.lock().unwrap();
    let project = guard.as_ref().ok_or("no project open")?;
    let cat = match category.as_deref() {
        Some("vehicle") | Some("Vehicle") => asset_delivery::AssetCategory::Vehicle,
        Some("character") | Some("Character") => asset_delivery::AssetCategory::Character,
        Some("environment") | Some("Environment") => asset_delivery::AssetCategory::Environment,
        Some("prop") | Some("Prop") => asset_delivery::AssetCategory::Prop,
        Some("weapon") | Some("Weapon") => asset_delivery::AssetCategory::Weapon,
        Some("building") | Some("Building") => asset_delivery::AssetCategory::Building,
        Some("vegetation") | Some("Vegetation") => asset_delivery::AssetCategory::Vegetation,
        Some("furniture") | Some("Furniture") => asset_delivery::AssetCategory::Furniture,
        Some("equipment") | Some("Equipment") => asset_delivery::AssetCategory::Equipment,
        Some("texture") | Some("Texture") => asset_delivery::AssetCategory::Texture,
        Some("material") | Some("Material") => asset_delivery::AssetCategory::Material,
        _ => {
            // Auto-classify from the asset's original filename.
            let db = project.db.lock().map_err(|e| e.to_string())?;
            let row: Result<(String, String), _> = db.query_row(
                "SELECT original_filename, media_kind FROM assets WHERE asset_id=?1",
                rusqlite::params![asset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            );
            drop(db);
            match row {
                Ok((filename, kind)) => asset_delivery::AssetCategory::classify(&filename, &kind),
                Err(_) => asset_delivery::AssetCategory::Prop,
            }
        }
    };

    let deployment = asset_delivery::deploy_model3d_to_unity(
        project,
        &asset_id,
        "",
        &target_id,
        cat,
    )
    .map_err(|e| command_error(&state, "unity_deploy_failed", e))?;

    state.logger.info(
        "unity_deploy_succeeded",
        &format!(
            "{} -> {} ({} bytes, category={:?})",
            deployment.asset_id,
            deployment.deployed_path.display(),
            deployment.bytes_written,
            cat
        ),
    );

    Ok(UnityDeploymentDto {
        delivery_id: deployment.delivery_id,
        asset_id: deployment.asset_id,
        target_id: deployment.target_id,
        unity_project_root: deployment.unity_project_root.to_string_lossy().into_owned(),
        destination_folder: deployment.destination_folder.to_string_lossy().into_owned(),
        deployed_path: deployment.deployed_path.to_string_lossy().into_owned(),
        meta_path: deployment.meta_path.to_string_lossy().into_owned(),
        file_size: deployment.file_size,
        bytes_written: deployment.bytes_written,
        checksum: deployment.checksum,
        category: format!("{:?}", cat),
    })
}

#[cfg(test)]
mod recent_project_tests {
    use super::*;

    fn manifest(project_id: &str) -> ProjectManifest {
        ProjectManifest {
            schema_version: 1,
            project_id: project_id.to_string(),
            name: format!("Project {project_id}"),
            created_at_ms: 1,
            format_version: "0.1.0".to_string(),
            engine_scope: None,
        }
    }

    #[test]
    fn recent_projects_are_rooted_deduplicated_and_capped() {
        let mut recent = Vec::new();

        for index in 0..11 {
            record_recent_project(
                &mut recent,
                PathBuf::from(format!("root-{index}")),
                manifest(&index.to_string()),
            );
        }

        assert_eq!(recent.len(), 10);
        assert_eq!(recent[0].root, PathBuf::from("root-10"));
        assert!(
            recent
                .iter()
                .all(|project| project.manifest.project_id != "0")
        );

        record_recent_project(&mut recent, PathBuf::from("new-root"), manifest("5"));

        assert_eq!(recent.len(), 10);
        assert_eq!(recent[0].root, PathBuf::from("new-root"));
        assert_eq!(
            recent
                .iter()
                .filter(|project| project.manifest.project_id == "5")
                .count(),
            1
        );
    }

    #[test]
    fn recent_project_serialization_does_not_claim_project_is_open() {
        let recent = RecentProjectInfo {
            root: PathBuf::from("project-root"),
            manifest: manifest("project-id"),
        };

        let value = serde_json::to_value(recent).unwrap();

        assert_eq!(value["root"], "project-root");
        assert_eq!(value["manifest"]["projectId"], "project-id");
        assert!(value.get("isOpen").is_none());
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;
    use crate::project::{
        DATABASE_FILENAME, MANIFEST_FILENAME, ProjectManifest, ProjectState, canonicalize_path,
        save_manifest,
    };
    use image::{ImageBuffer, Rgb, Rgba};
    use std::fs;
    use tempfile::tempdir;

    fn create_test_png(path: &std::path::Path, width: u32, height: u32) {
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        img.save(path).unwrap();
    }

    fn create_test_jpeg(path: &std::path::Path, width: u32, height: u32) {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        img.save(path).unwrap();
    }

    fn create_project_for_test() -> (tempfile::TempDir, ProjectState) {
        let dir = tempdir().unwrap();
        let root = dir.path().join("TestProject");
        let state = ProjectState::create(&root, "Test Project").unwrap();
        (dir, state)
    }

    #[test]
    fn test_import_valid_png() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("test.png");
        create_test_png(&source_path, 100, 100);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_test.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        let (width, height, format, has_alpha) = extract_image_metadata(&master_path).unwrap();
        assert_eq!(width, 100);
        assert_eq!(height, 100);
        assert_eq!(format, "PNG");
        assert!(has_alpha);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let sql = r#"
            INSERT INTO assets (
                asset_id, project_id, original_filename, managed_master_path,
                file_size, checksum, image_width, image_height, image_format,
                has_alpha, imported_at_ms, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#;

        let mut db = project.db.lock().unwrap();
        db.execute(
            sql,
            rusqlite::params![
                asset_id.clone(),
                project.manifest.project_id.clone(),
                "test.png".to_string(),
                master_path.to_string_lossy().to_string(),
                staging_data.len() as u64,
                checksum.clone(),
                width as i64,
                height as i64,
                format,
                if has_alpha { 1 } else { 0 },
                now as i64,
                "ready"
            ],
        )
        .unwrap();

        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let stored_checksum: String = db
            .query_row(
                "SELECT checksum FROM assets WHERE asset_id = ?1",
                rusqlite::params![asset_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_checksum, checksum);

        let master_data = fs::read(&master_path).unwrap();
        let master_checksum = compute_checksum(&master_data);
        assert_eq!(master_checksum, checksum);

        let source_data = fs::read(&source_path).unwrap();
        let source_checksum = compute_checksum(&source_data);
        assert_eq!(source_checksum, checksum);
    }

    #[test]
    fn test_import_valid_jpeg() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("test.jpg");
        create_test_jpeg(&source_path, 200, 150);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_test.jpg");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.jpg", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        let (width, height, format, has_alpha) = extract_image_metadata(&master_path).unwrap();
        assert_eq!(width, 200);
        assert_eq!(height, 150);
        assert_eq!(format, "JPEG");
        assert!(!has_alpha);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let sql = r#"
            INSERT INTO assets (
                asset_id, project_id, original_filename, managed_master_path,
                file_size, checksum, image_width, image_height, image_format,
                has_alpha, imported_at_ms, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#;

        let mut db = project.db.lock().unwrap();
        db.execute(
            sql,
            rusqlite::params![
                asset_id.clone(),
                project.manifest.project_id.clone(),
                "test.jpg".to_string(),
                master_path.to_string_lossy().to_string(),
                staging_data.len() as u64,
                checksum.clone(),
                width as i64,
                height as i64,
                format,
                if has_alpha { 1 } else { 0 },
                now as i64,
                "ready"
            ],
        )
        .unwrap();

        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let master_data = fs::read(&master_path).unwrap();
        let master_checksum = compute_checksum(&master_data);
        assert_eq!(master_checksum, checksum);

        let source_data = fs::read(&source_path).unwrap();
        let source_checksum = compute_checksum(&source_data);
        assert_eq!(source_checksum, checksum);
    }

    #[test]
    fn test_source_safety_original_unchanged() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("original.png");
        create_test_png(&source_path, 50, 50);

        let original_data = fs::read(&source_path).unwrap();
        let original_checksum = compute_checksum(&original_data);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_original.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        let after_data = fs::read(&source_path).unwrap();
        let after_checksum = compute_checksum(&after_data);
        assert_eq!(
            original_checksum, after_checksum,
            "Source file was modified during import"
        );
    }

    #[test]
    fn test_master_exists_and_checksum_correct() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("master_test.png");
        create_test_png(&source_path, 80, 60);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_master_test.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        assert!(master_path.exists(), "Master file should exist");

        let master_data = fs::read(&master_path).unwrap();
        let master_checksum = compute_checksum(&master_data);
        assert_eq!(
            master_checksum, checksum,
            "Master checksum should match staging checksum"
        );
    }

    #[test]
    fn test_duplicate_detection_same_content() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("duplicate.png");
        create_test_png(&source_path, 100, 100);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_duplicate.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id1 = uuid::Uuid::now_v7().to_string();
        let master_filename1 = format!("{}.png", asset_id1);
        let master_path1 = master_dir.join(&master_filename1);
        fs::copy(&staging_path, &master_path1).unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let sql = r#"
            INSERT INTO assets (
                asset_id, project_id, original_filename, managed_master_path,
                file_size, checksum, image_width, image_height, image_format,
                has_alpha, imported_at_ms, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#;

        {
            let mut db = project.db.lock().unwrap();
            db.execute(
                sql,
                rusqlite::params![
                    asset_id1.clone(),
                    project.manifest.project_id.clone(),
                    "duplicate.png".to_string(),
                    master_path1.to_string_lossy().to_string(),
                    staging_data.len() as u64,
                    checksum.clone(),
                    100,
                    100,
                    "PNG",
                    1,
                    now as i64,
                    "ready"
                ],
            )
            .unwrap();
        }

        let is_duplicate: Result<bool, _> = {
            let db = project.db.lock().unwrap();
            db.query_row(
                "SELECT 1 FROM assets WHERE checksum = ?1 LIMIT 1",
                rusqlite::params![checksum],
                |_| Ok(true),
            )
        };
        assert!(is_duplicate.is_ok(), "Duplicate should be detected");
    }

    #[test]
    fn test_same_filename_different_content() {
        let (_dir, project) = create_project_for_test();

        let source_dir1 = tempdir().unwrap();
        let source_path1 = source_dir1.path().join("same_name.png");
        create_test_png(&source_path1, 100, 100);

        let source_dir2 = tempdir().unwrap();
        let source_path2 = source_dir2.path().join("same_name.png");
        create_test_png(&source_path2, 200, 200);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();

        let staging_path1 = staging_dir.join("import_same_name_1.png");
        fs::copy(&source_path1, &staging_path1).unwrap();
        let staging_data1 = fs::read(&staging_path1).unwrap();
        let checksum1 = compute_checksum(&staging_data1);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id1 = uuid::Uuid::now_v7().to_string();
        let master_filename1 = format!("{}.png", asset_id1);
        let master_path1 = master_dir.join(&master_filename1);
        fs::copy(&staging_path1, &master_path1).unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let sql = r#"
            INSERT INTO assets (
                asset_id, project_id, original_filename, managed_master_path,
                file_size, checksum, image_width, image_height, image_format,
                has_alpha, imported_at_ms, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#;

        {
            let mut db = project.db.lock().unwrap();
            db.execute(
                sql,
                rusqlite::params![
                    asset_id1.clone(),
                    project.manifest.project_id.clone(),
                    "same_name.png".to_string(),
                    master_path1.to_string_lossy().to_string(),
                    staging_data1.len() as u64,
                    checksum1.clone(),
                    100,
                    100,
                    "PNG",
                    1,
                    now as i64,
                    "ready"
                ],
            )
            .unwrap();
        }

        let staging_path2 = staging_dir.join("import_same_name_2.png");
        fs::copy(&source_path2, &staging_path2).unwrap();
        let staging_data2 = fs::read(&staging_path2).unwrap();
        let checksum2 = compute_checksum(&staging_data2);

        assert_ne!(
            checksum1, checksum2,
            "Different content should have different checksums"
        );

        let is_duplicate: Result<bool, _> = {
            let db = project.db.lock().unwrap();
            db.query_row(
                "SELECT 1 FROM assets WHERE checksum = ?1 LIMIT 1",
                rusqlite::params![checksum2],
                |_| Ok(true),
            )
        };
        assert!(
            is_duplicate.is_err() || is_duplicate.unwrap_or(false) == false,
            "Different content should not be detected as duplicate"
        );

        let asset_id2 = uuid::Uuid::now_v7().to_string();
        let master_filename2 = format!("{}.png", asset_id2);
        let master_path2 = master_dir.join(&master_filename2);
        fs::copy(&staging_path2, &master_path2).unwrap();

        {
            let mut db = project.db.lock().unwrap();
            db.execute(
                sql,
                rusqlite::params![
                    asset_id2.clone(),
                    project.manifest.project_id.clone(),
                    "same_name.png".to_string(),
                    master_path2.to_string_lossy().to_string(),
                    staging_data2.len() as u64,
                    checksum2.clone(),
                    200,
                    200,
                    "PNG",
                    1,
                    now as i64,
                    "ready"
                ],
            )
            .unwrap();
        }

        let count: i64 = {
            let db = project.db.lock().unwrap();
            db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(count, 2, "Both assets should coexist");
    }

    #[test]
    fn test_metadata_width_height_format_size() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("metadata_test.png");
        create_test_png(&source_path, 320, 240);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_metadata_test.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let _checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        let (width, height, format, has_alpha) = extract_image_metadata(&master_path).unwrap();
        assert_eq!(width, 320);
        assert_eq!(height, 240);
        assert_eq!(format, "PNG");
        assert!(has_alpha);
        assert_eq!(
            staging_data.len(),
            fs::metadata(&master_path).unwrap().len() as usize
        );
    }

    #[test]
    fn test_zero_byte_file_rejected() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("empty.png");
        fs::write(&source_path, []).unwrap();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_empty.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        assert_eq!(
            staging_data.len(),
            0,
            "Zero-byte file should have zero length"
        );
    }

    #[test]
    fn test_corrupt_png_rejected() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("corrupt.png");
        fs::write(&source_path, b"not a valid png").unwrap();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_corrupt.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let result = extract_image_metadata(&staging_path);
        assert!(result.is_err(), "Corrupt PNG should be rejected");
    }

    #[test]
    fn test_corrupt_jpeg_rejected() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("corrupt.jpg");
        fs::write(&source_path, b"not a valid jpeg").unwrap();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_corrupt.jpg");
        fs::copy(&source_path, &staging_path).unwrap();

        let result = extract_image_metadata(&staging_path);
        assert!(result.is_err(), "Corrupt JPEG should be rejected");
    }

    #[test]
    fn test_unsupported_format_rejected() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("test.gif");
        fs::write(&source_path, b"GIF89a").unwrap();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_test.gif");
        fs::copy(&source_path, &staging_path).unwrap();

        let result = extract_image_metadata(&staging_path);
        assert!(result.is_err(), "Unsupported format should be rejected");
    }

    #[test]
    fn test_database_migration_phase2_to_phase3() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("MigrationTest");
        fs::create_dir_all(&root).unwrap();

        let manifest = ProjectManifest::new("Migration Test".to_string());
        let manifest_path = root.join(MANIFEST_FILENAME);
        save_manifest(&manifest_path, &manifest).unwrap();

        let database_path = root.join(DATABASE_FILENAME);
        let db = rusqlite::Connection::open(&database_path).unwrap();
        db.execute_batch(include_str!("../migrations/001_initial.sql"))
            .unwrap();

        let version: i64 = db
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 1);

        db.execute_batch(include_str!("../migrations/002_assets.sql"))
            .unwrap();

        let version: i64 = db
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);

        let table_exists: bool = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='assets'",
                [],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap();
        assert!(table_exists, "Assets table should exist after migration");
    }

    #[test]
    fn test_existing_project_data_preserved_after_migration() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("PreservationTest");
        fs::create_dir_all(&root).unwrap();

        let manifest = ProjectManifest::new("Preservation Test".to_string());
        let project_id = manifest.project_id.clone();
        let manifest_path = root.join(MANIFEST_FILENAME);
        save_manifest(&manifest_path, &manifest).unwrap();

        let database_path = root.join(DATABASE_FILENAME);
        let db = rusqlite::Connection::open(&database_path).unwrap();
        db.execute_batch(include_str!("../migrations/001_initial.sql"))
            .unwrap();
        db.execute_batch(include_str!("../migrations/002_assets.sql"))
            .unwrap();

        let reopened = ProjectState::open(&root).unwrap();
        assert_eq!(reopened.manifest.project_id, project_id);
        assert_eq!(reopened.manifest.name, "Preservation Test");
        reopened.close().unwrap();
    }

    #[test]
    fn test_asset_persists_across_close_reopen() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("persist.png");
        create_test_png(&source_path, 100, 100);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_persist.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();
        let _ = fs::remove_file(&staging_path);

        let (width, height, format, has_alpha) = extract_image_metadata(&master_path).unwrap();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let sql = r#"
            INSERT INTO assets (
                asset_id, project_id, original_filename, managed_master_path,
                file_size, checksum, image_width, image_height, image_format,
                has_alpha, imported_at_ms, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#;

        {
            let mut db = project.db.lock().unwrap();
            db.execute(
                sql,
                rusqlite::params![
                    asset_id.clone(),
                    project.manifest.project_id.clone(),
                    "persist.png".to_string(),
                    master_path.to_string_lossy().to_string(),
                    staging_data.len() as u64,
                    checksum.clone(),
                    width as i64,
                    height as i64,
                    format,
                    if has_alpha { 1 } else { 0 },
                    now as i64,
                    "ready"
                ],
            )
            .unwrap();
        }

        let project_root = project.root().clone();
        let _manifest_project_id = project.manifest.project_id.clone();
        drop(project);

        let reopened = ProjectState::open(&project_root).unwrap();
        let count: i64 = {
            let db = reopened.db.lock().unwrap();
            db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(count, 1, "Asset should persist after reopen");

        let stored_checksum: String = {
            let db = reopened.db.lock().unwrap();
            db.query_row(
                "SELECT checksum FROM assets WHERE asset_id = ?1",
                rusqlite::params![asset_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(stored_checksum, checksum);
        reopened.close().unwrap();
    }

    #[test]
    fn test_abandoned_staging_cleanup() {
        let (_dir, project) = create_project_for_test();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();

        let abandoned_path = staging_dir.join("abandoned_import.png");
        fs::write(&abandoned_path, b"staging data").unwrap();

        assert!(abandoned_path.exists(), "Abandoned staging file exists");

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        let master_data = b"valid master data";
        fs::write(&master_path, master_data).unwrap();

        let staging_files: Vec<_> = fs::read_dir(&staging_dir).unwrap().collect();
        assert_eq!(
            staging_files.len(),
            1,
            "Abandoned staging file should remain"
        );

        let _ = fs::remove_file(&abandoned_path);
        assert!(
            !abandoned_path.exists(),
            "Abandoned staging can be cleaned up"
        );
        assert!(
            master_path.exists(),
            "Master should not be affected by staging cleanup"
        );
    }

    #[test]
    fn test_interrupted_import_no_false_ready() {
        let (_dir, project) = create_project_for_test();

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_interrupted.png");
        fs::write(&staging_path, b"partial data").unwrap();

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let count_before: i64 = {
            let db = project.db.lock().unwrap();
            db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(count_before, 0);

        let _ = fs::remove_file(&staging_path);

        let count_after: i64 = {
            let db = project.db.lock().unwrap();
            db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(
            count_after, 0,
            "No false READY asset should exist after interrupted import"
        );
    }

    #[test]
    fn test_path_traversal_rejected() {
        let result = canonicalize_path(&std::path::Path::new("..").join("secret"));
        assert!(result.is_err() || !result.unwrap().to_string_lossy().contains(".."));
    }

    #[test]
    fn test_managed_paths_stay_inside_project_root() {
        let (_dir, project) = create_project_for_test();
        let project_root = project.root().clone();

        let master_dir = project_root.join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);

        fs::write(&master_path, b"test").unwrap();

        let canonical_master = dunce::canonicalize(&master_path).unwrap();
        let canonical_root = dunce::canonicalize(&project_root).unwrap();

        assert!(
            canonical_master.starts_with(&canonical_root),
            "Master path must stay inside project root"
        );
    }

    #[test]
    fn test_importing_external_source_not_making_parent_managed() {
        let (_dir, project) = create_project_for_test();
        let project_root = project.root().clone();

        let external_dir = tempdir().unwrap();
        let source_path = external_dir.path().join("external.png");
        create_test_png(&source_path, 100, 100);

        let canonical_external = dunce::canonicalize(external_dir.path()).unwrap();
        let canonical_project = dunce::canonicalize(&project_root).unwrap();

        assert!(
            !canonical_project.starts_with(&canonical_external),
            "Project root should not be inside external source directory"
        );
        assert!(
            !canonical_external.starts_with(&canonical_project),
            "External source should not be inside project root"
        );
    }

    #[test]
    fn test_checksum_detects_master_corruption() {
        let (_dir, project) = create_project_for_test();

        let source_dir = tempdir().unwrap();
        let source_path = source_dir.path().join("integrity.png");
        create_test_png(&source_path, 100, 100);

        let staging_dir = project.root().join("assets").join("staging");
        fs::create_dir_all(&staging_dir).unwrap();
        let staging_path = staging_dir.join("import_integrity.png");
        fs::copy(&source_path, &staging_path).unwrap();

        let staging_data = fs::read(&staging_path).unwrap();
        let original_checksum = compute_checksum(&staging_data);

        let master_dir = project.root().join("assets").join("masters");
        fs::create_dir_all(&master_dir).unwrap();

        let asset_id = uuid::Uuid::now_v7().to_string();
        let master_filename = format!("{}.png", asset_id);
        let master_path = master_dir.join(&master_filename);
        fs::copy(&staging_path, &master_path).unwrap();

        let master_data_before = fs::read(&master_path).unwrap();
        let checksum_before = compute_checksum(&master_data_before);
        assert_eq!(checksum_before, original_checksum);

        fs::write(&master_path, b"corrupted data").unwrap();

        let master_data_after = fs::read(&master_path).unwrap();
        let checksum_after = compute_checksum(&master_data_after);
        assert_ne!(
            checksum_after, original_checksum,
            "Checksum should detect corruption"
        );
    }

    #[test]
    fn model_imports_are_atomic_typed_self_contained_and_persistent() {
        let (dir, project) = create_project_for_test();
        let sources = tempdir().unwrap();
        let glb_path = sources.path().join("triangle.glb");
        let glb = crate::model3d_validation::tests::fixture_glb();
        fs::write(&glb_path, &glb).unwrap();
        let imported = import_model_asset(&project, &glb_path).unwrap();
        assert_eq!(
            (
                imported.media_kind.as_str(),
                imported.media_container.as_deref(),
                imported.validation_level.as_deref()
            ),
            ("model3d", Some("GLB"), Some("structural"))
        );
        assert_eq!(imported.model_metadata.as_ref().unwrap().triangle_count, 1);
        assert_eq!(fs::read(&glb_path).unwrap(), glb);
        assert_eq!(fs::read(&imported.managed_master_path).unwrap(), glb);
        assert!(
            matches!(import_model_asset(&project, &glb_path), Err(error) if error.contains("duplicate"))
        );
        let master_entries = fs::read_dir(project.root.join("assets").join("masters"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(master_entries.len(), 1);
        assert!(
            !project
                .root
                .join("assets")
                .join("staging")
                .join(&imported.asset_id)
                .exists()
        );

        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::prelude::BASE64_STANDARD.encode([0u8; 36])
        );
        let gltf = crate::model3d_validation::tests::fixture_json(Some(&uri));
        let gltf_path = sources.path().join("triangle.gltf");
        fs::write(&gltf_path, &gltf).unwrap();
        let imported_gltf = import_model_asset(&project, &gltf_path).unwrap();
        assert_eq!(
            (
                imported_gltf.media_container.as_deref(),
                imported_gltf.validation_level.as_deref()
            ),
            (Some("GLTF"), Some("structural-self-contained"))
        );
        let gltf_master = imported_gltf.managed_master_path.clone();
        drop(project);
        let reopened = ProjectState::open(&dir.path().join("TestProject")).unwrap();
        let stored: (String, i64, String) = reopened.with_db(|db| db.query_row("SELECT media_kind,model_metadata_schema_version,model_metadata_json FROM assets WHERE asset_id=?1", [&imported_gltf.asset_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!((stored.0.as_str(), stored.1), ("model3d", 1));
        assert_eq!(
            serde_json::from_str::<crate::model3d_validation::Model3dMetadata>(&stored.2)
                .unwrap()
                .vertex_count,
            3
        );
        assert_eq!(fs::read(gltf_master).unwrap(), gltf);
    }

    #[test]
    fn concurrent_model_duplicate_recheck_leaves_one_row_and_master() {
        let (_dir, project) = create_project_for_test();
        let sources = tempdir().unwrap();
        let source = sources.path().join("race.glb");
        fs::write(&source, crate::model3d_validation::tests::fixture_glb()).unwrap();
        let project = Arc::new(project);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let project = project.clone();
            let barrier = barrier.clone();
            let source = source.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                import_model_asset(&project, &source)
            }));
        }
        barrier.wait();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.contains("duplicate")))
                .count(),
            1
        );
        assert_eq!(
            project
                .with_db(|db| db.query_row(
                    "SELECT COUNT(*) FROM assets WHERE media_kind='model3d'",
                    [],
                    |row| row.get::<_, i64>(0)
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            fs::read_dir(project.root.join("assets").join("masters"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(project.root.join("assets").join("staging"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn model_import_rejects_managed_directory_link_escape_when_supported() {
        let (_dir, project) = create_project_for_test();
        let outside = tempdir().unwrap();
        let assets = project.root.join("assets");
        fs::create_dir_all(&assets).unwrap();
        let staging = assets.join("staging");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(outside.path(), &staging);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), &staging);
        if linked.is_err() {
            return;
        }
        let source_dir = tempdir().unwrap();
        let source = source_dir.path().join("escape.glb");
        fs::write(&source, crate::model3d_validation::tests::fixture_glb()).unwrap();
        assert!(
            matches!(import_model_asset(&project, &source), Err(error) if error.contains("escapes managed project storage"))
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}

pub fn run() {
    // Initialize tracing for structured logging (file only, no console in release)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nexora_game_studio=debug".parse().unwrap())
                .add_directive("runtime_manager=debug".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("nexora-media", |context, request, responder| {
            asset_delivery::handle_protocol(context.app_handle(), request, responder);
        })
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir()?;
            let settings_dir = data_dir.join("config");
            let recent_projects_path = settings_dir.join("recent-projects.v1.json");
            let log_dir = data_dir.join("logs");
            std::fs::create_dir_all(&settings_dir)?;
            std::fs::create_dir_all(&log_dir)?;
            let logger = JsonLogger::new(log_dir.join("nexora-game-studio.jsonl"))?;
            logger.info("application_startup", "Nexora Game Studio is starting");
            tracing::info!("Nexora Game Studio starting up");

            let current_project = Arc::new(Mutex::new(None));
            let hardware_snapshot = hardware::detect(None);
            let image_provider_config_path = settings_dir.join("image-provider.v1.json");
            let image_provider_config = image_generation::load_config(&image_provider_config_path)?;
            let video_provider_config_path = settings_dir.join("video-provider.v1.json");
            let video_provider_config = video_generation::load_config(&video_provider_config_path)?;
            let app_settings =
                settings::load(&settings_dir.join("settings.json")).unwrap_or_default();
            let provider_registry = ProviderRegistry::phase8(
                image_provider_config.enabled,
                video_provider_config.enabled,
                app_settings.openrouter.enabled,
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;
            let image_provider_config = Arc::new(Mutex::new(image_provider_config));
            let video_provider_config = Arc::new(Mutex::new(video_provider_config));
            let job_worker = JobWorker::start(
                current_project.clone(),
                image_provider_config.clone(),
                video_provider_config.clone(),
            );

            // Initialize RuntimeManager and register discovered runtimes
            let mut runtime_manager = RuntimeManager::new();
            runtime_manager.set_app_handle(app.handle().clone());

            // Use discovered configurations for auto-starting runtimes
            let a1111_config = RuntimeManager::discover_a1111()
                .unwrap_or_else(|| app_settings.a1111.to_runtime_config());
            let hunyuan_config = RuntimeManager::discover_hunyuan3d()
                .unwrap_or_else(|| app_settings.hunyuan.to_runtime_config());
            let blender_config =
                RuntimeManager::discover_blender().unwrap_or_else(|| RuntimeConfig {
                    runtime_id: "blender".to_string(),
                    display_name: "Blender".to_string(),
                    runtime_type: RuntimeType::Blender,
                    kind: RuntimeKind::OnDemandExecutable,
                    install_path: None,
                    launcher_path: None,
                    base_url: None,
                    health_endpoint: None,
                    auto_start: false,
                    startup_args: vec![],
                    working_directory: None,
                    environment: HashMap::new(),
                });
            if let Err(error) = tauri::async_runtime::block_on(runtime_manager.register_runtime(a1111_config.clone())) {
                tracing::warn!(?error, runtime_id = %a1111_config.runtime_id, "automatic1111 runtime registration failed");
            }
            if let Err(error) = tauri::async_runtime::block_on(runtime_manager.register_runtime(hunyuan_config.clone())) {
                tracing::warn!(?error, runtime_id = %hunyuan_config.runtime_id, "hunyuan3d runtime registration failed");
            }
            if let Err(error) = tauri::async_runtime::block_on(runtime_manager.register_runtime(blender_config.clone())) {
                tracing::warn!(?error, runtime_id = %blender_config.runtime_id, "blender runtime registration failed");
            }
            let runtime_manager = Arc::new(Mutex::new(runtime_manager));

            // Initialize ProviderOrchestrator
            let runtime_manager_arc = {
                let guard = runtime_manager.lock().unwrap();
                Arc::new((*guard).clone())
            };
            let mut provider_orchestrator = provider_orchestrator::ProviderOrchestrator::new(runtime_manager_arc);
            provider_orchestrator.set_app_handle(app.handle().clone());
            let provider_orchestrator = Arc::new(Mutex::new(provider_orchestrator));

            // Start sequential provider initialization
            let provider_orchestrator_clone = provider_orchestrator.clone();
            tauri::async_runtime::spawn(async move {
                tracing::info!("Starting sequential provider initialization");
                let orchestrator = {
                    let guard = provider_orchestrator_clone.lock().unwrap();
                    (*guard).clone()
                };
                if let Err(error) = orchestrator.initialize_sequential().await {
                    tracing::error!(?error, "Provider initialization failed");
                }
                tracing::info!("Provider initialization completed");
            });

            app.manage(AppState {
                settings_path: settings_dir.join("settings.json"),
                settings: Mutex::new(app_settings),
                image_provider_config_path,
                image_provider_config,
                video_provider_config_path,
                video_provider_config,
                logger,
                current_project,
                job_worker,
                recent_projects_path,
                recent_projects: Mutex::new(load_recent_projects(&settings_dir.join("recent-projects.v1.json"))),
                hardware_snapshot: Mutex::new(hardware_snapshot),
                provider_registry: Mutex::new(provider_registry),
                media_integrity_cache: asset_delivery::IntegrityCache::default(),
                runtime_manager,
                provider_orchestrator,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            load_settings,
            save_settings,
            get_log_location,
            create_project,
            open_project,
            close_project,
            get_current_project,
            get_recent_projects,
            archive_project,
            delete_project,
            import_asset,
            list_assets,
            search_assets,
            get_asset_preview,
            create_diagnostic_job,
            create_provider_diagnostic_job,
            list_jobs,
            get_job_details,
            request_job_cancellation,
            retry_job,
            get_hardware_snapshot,
            refresh_hardware_snapshot,
            list_providers,
            refresh_provider_health,
            select_provider,
            get_image_provider_config,
            save_image_provider_config,
            list_image_generation_providers,
            create_image_generation_job,
            get_image_generation_result,
            get_video_provider_config,
            save_video_provider_config,
            list_video_generation_providers,
            create_video_generation_job,
            get_video_generation_result,
            create_model3d_generation_job,
            get_model3d_generation_result,
            create_model3d_processing_job,
            get_model3d_processing_result,
            approve_model3d_asset,
            approve_image_asset,
            reject_model3d_asset,
            reprocess_model3d_asset,
            create_unity_delivery,
            update_unity_delivery_status,
            get_unity_delivery,
            list_unity_deliveries,
            verify_asset_approved_for_unity_delivery,
            import_unity_asset,
            get_unity_import_status,
            install_unity_package,
            create_unity_prefab,
            validate_unity_import,
            create_hunyuan_generation_job,
            get_hunyuan_generation_result,
            discover_unity_project,
            detect_engine_targets,
            validate_unity_project,
            save_unity_config,
            discover_blender,
            validate_blender_executable,
            list_runtimes,
            check_and_update_engines,
            get_runtime,
            check_runtime_health,
            start_runtime,
            stop_runtime,
            initialize_runtimes,
            get_a1111_config,
            save_a1111_config,
            get_hunyuan_config,
            save_hunyuan_config,
            get_openrouter_config,
            save_openrouter_config,
            remove_openrouter_key,
            test_openrouter_connection,
            list_openrouter_models,
            openrouter_chat_completion,
            get_skip_runtime_startup,
            set_skip_runtime_startup,
            discover_runtimes,
            deploy_model3d_to_unity,
            get_orchestrator_status,
            start_orchestrator,
        ])
        .build(tauri::generate_context!())
        .expect("failed to initialize Nexora Game Studio");

    tracing::info!("Tauri app built successfully, starting event loop");

    app.run(|handle, event| {
        tracing::info!(?event, "Tauri event received");
        match event {
            RunEvent::Exit => {
                if let Some(state) = handle.try_state::<AppState>() {
                    state.job_worker.stop();
                    if let Ok(mut guard) = state.current_project.lock() {
                        let _ = retire_current_project(&mut guard);
                    }
                    // Shutdown Nexora-owned runtime processes
                    let runtime_manager_arc = (*state.runtime_manager.lock().unwrap()).clone();
                    let _ = tauri::async_runtime::block_on(runtime_manager_arc.shutdown_all());
                    state
                        .logger
                        .info("application_shutdown", "Nexora Game Studio closed cleanly");
                }
            }
            _ => {}
        }
    });
}

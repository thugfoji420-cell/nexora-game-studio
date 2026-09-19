use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::Emitter;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use which::which;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeType {
    Automatic1111,
    Hunyuan3D,
    Blender,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    NotConfigured,
    NotInstalled,
    Stopped,
    Starting,
    Ready,
    Degraded,
    Failed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    LongRunningService,
    OnDemandExecutable,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConfig {
    pub runtime_id: String,
    pub display_name: String,
    pub runtime_type: RuntimeType,
    pub kind: RuntimeKind,
    pub install_path: Option<String>,
    pub launcher_path: Option<String>,
    pub base_url: Option<String>,
    pub health_endpoint: Option<String>,
    pub auto_start: bool,
    pub startup_args: Vec<String>,
    pub working_directory: Option<String>,
    pub environment: HashMap<String, String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            runtime_id: String::new(),
            display_name: String::new(),
            runtime_type: RuntimeType::Automatic1111,
            kind: RuntimeKind::LongRunningService,
            install_path: None,
            launcher_path: None,
            base_url: None,
            health_endpoint: None,
            auto_start: false,
            startup_args: Vec::new(),
            working_directory: None,
            environment: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub config: RuntimeConfig,
    pub status: RuntimeStatus,
    pub started_by_nexora: bool,
    pub process_id: Option<u32>,
    pub last_health_check: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub startup_timestamp: Option<DateTime<Utc>>,
    pub readiness_time_ms: Option<u64>,
}

impl RuntimeState {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            status: RuntimeStatus::NotConfigured,
            started_by_nexora: false,
            process_id: None,
            last_health_check: None,
            error: None,
            startup_timestamp: None,
            readiness_time_ms: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime not found: {0}")]
    NotFound(String),
    #[error("runtime already running")]
    AlreadyRunning,
    #[error("runtime not started by nexora")]
    NotOwned,
    #[error("process spawn failed: {0}")]
    ProcessSpawn(String),
    #[error("health check failed: {0}")]
    HealthCheck(String),
    #[error("timeout waiting for readiness")]
    Timeout,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Clone)]
pub struct RuntimeManager {
    runtimes: Arc<AsyncMutex<HashMap<String, RuntimeState>>>,
    child_processes: Arc<AsyncMutex<HashMap<String, std::process::Child>>>,
    app_handle: Option<tauri::AppHandle>,
}

impl RuntimeManager {
    pub fn new() -> Self {
        Self {
            runtimes: Arc::new(AsyncMutex::new(HashMap::new())),
            child_processes: Arc::new(AsyncMutex::new(HashMap::new())),
            app_handle: None,
        }
    }

    pub fn set_app_handle(&mut self, handle: tauri::AppHandle) {
        self.app_handle = Some(handle);
    }

    fn emit_status_changed(&self, runtime_id: &str, status: RuntimeStatus) {
        if let Some(handle) = &self.app_handle {
            let _ = handle.emit(
                "runtime://status-changed",
                serde_json::json!({
                    "runtimeId": runtime_id,
                    "status": status,
                }),
            );
        }
    }

    fn emit_init_log(&self, level: &str, message: &str, runtime_id: Option<&str>) {
        if let Some(handle) = &self.app_handle {
            let _ = handle.emit(
                "runtime://init-log",
                serde_json::json!({
                    "timestamp": Utc::now().to_rfc3339(),
                    "level": level,
                    "message": message,
                    "runtimeId": runtime_id,
                }),
            );
        }
    }

    pub async fn register_runtime(&self, config: RuntimeConfig) -> Result<(), RuntimeError> {
        let mut runtimes = self.runtimes.lock().await;
        if runtimes.contains_key(&config.runtime_id) {
            return Err(RuntimeError::AlreadyRunning);
        }
        let state = RuntimeState::new(config);
        runtimes.insert(state.config.runtime_id.clone(), state);
        Ok(())
    }

    pub async fn get_runtime(&self, runtime_id: &str) -> Result<RuntimeState, RuntimeError> {
        let runtimes = self.runtimes.lock().await;
        runtimes
            .get(runtime_id)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(runtime_id.to_string()))
    }

    pub async fn list_runtimes(&self) -> Vec<RuntimeState> {
        let runtimes = self.runtimes.lock().await;
        runtimes.values().cloned().collect()
    }

    pub async fn update_runtime_state<F>(
        &self,
        runtime_id: &str,
        f: F,
    ) -> Result<RuntimeState, RuntimeError>
    where
        F: FnOnce(&mut RuntimeState),
    {
        let mut runtimes = self.runtimes.lock().await;
        let state = runtimes
            .get_mut(runtime_id)
            .ok_or_else(|| RuntimeError::NotFound(runtime_id.to_string()))?;
        let old_status = state.status;
        f(state);
        let new_status = state.status;
        let runtime_id_owned = state.config.runtime_id.clone();
        if old_status != new_status {
            drop(runtimes);
            self.emit_status_changed(&runtime_id_owned, new_status);
            let runtimes = self.runtimes.lock().await;
            Ok(runtimes.get(runtime_id).cloned().unwrap())
        } else {
            Ok(state.clone())
        }
    }

    pub async fn check_health(&self, runtime_id: &str) -> Result<RuntimeStatus, RuntimeError> {
        let config = {
            let state = self.get_runtime(runtime_id).await?;
            state.config
        };

        self.emit_init_log("info", &format!("Checking {} health", config.display_name), Some(runtime_id));

        let status = match config.runtime_type {
            RuntimeType::Automatic1111 => self.check_a1111_health(&config).await,
            RuntimeType::Hunyuan3D => self.check_hunyuan_health(&config).await,
            RuntimeType::Blender => self.check_blender_health(&config).await,
        };

        let (new_status, error_str) = match &status {
            Ok(()) => {
                self.emit_init_log("info", &format!("{} health check passed", config.display_name), Some(runtime_id));
                (RuntimeStatus::Ready, None)
            }
            Err(e) => {
                let err_str = e.to_string();
                self.emit_init_log("error", &format!("{} health check failed: {}", config.display_name, err_str), Some(runtime_id));
                if err_str.contains("timeout") || err_str.contains("connection") {
                    (RuntimeStatus::Unavailable, Some(err_str))
                } else {
                    (RuntimeStatus::Failed, Some(err_str))
                }
            }
        };

        self.update_runtime_state(runtime_id, |state| {
            state.status = new_status;
            state.last_health_check = Some(Utc::now());
            state.error = error_str;
        })
        .await?;

        Ok(new_status)
    }

    async fn check_a1111_health(&self, config: &RuntimeConfig) -> Result<(), RuntimeError> {
        let base_url = config
            .base_url
            .as_deref()
            .unwrap_or("http://127.0.0.1:7860");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;

        let response = client
            .get(format!("{}/sdapi/v1/options", base_url))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(RuntimeError::HealthCheck(format!(
                "A1111 returned {}",
                response.status()
            )))
        }
    }

    async fn check_hunyuan_health(&self, config: &RuntimeConfig) -> Result<(), RuntimeError> {
        let base_url = config
            .base_url
            .as_deref()
            .unwrap_or("http://127.0.0.1:8081");

        // The /health endpoint can hang while the model is loading or under
        // load, even though the service is listening. Try the HTTP health
        // check first, but fall back to a TCP connect so a listening service
        // is still detected as ready instead of being marked offline.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        match client.get(format!("{}/health", base_url)).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => {
                return Err(RuntimeError::HealthCheck(format!(
                    "Hunyuan3D returned {}",
                    response.status()
                )))
            }
            Err(_) => {
                // HTTP check failed (timeout or connect error). Fall back to a
                // plain TCP connect — if the port answers, the service is up.
            }
        }

        let host_port = base_url
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let tcp_target = host_port.split('/').next().unwrap_or("127.0.0.1:8081");
        match tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(tcp_target),
        )
        .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => Err(RuntimeError::HealthCheck(format!(
                "Hunyuan3D port not reachable: {}",
                err
            ))),
            Err(_) => Err(RuntimeError::HealthCheck(
                "Hunyuan3D health check timed out".into(),
            )),
        }
    }

    async fn check_blender_health(&self, config: &RuntimeConfig) -> Result<(), RuntimeError> {
        let executable = config
            .launcher_path
            .as_deref()
            .or(config.install_path.as_deref())
            .ok_or_else(|| {
                RuntimeError::HealthCheck("Blender executable not configured".to_string())
            })?;

        let mut cmd = Command::new(executable);
        cmd.arg("--version");
        // These are long-running services. Do not leave piped streams unread:
        // Hunyuan3D emits enough startup logs to fill the OS pipe buffer and
        // stall before its HTTP server becomes ready. The process is tracked
        // below for shutdown, while its output is intentionally discarded.
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        #[cfg(windows)]
        cmd.creation_flags(0x08000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        let output = cmd.output()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::HealthCheck(
                "Blender --version failed".to_string(),
            ))
        }
    }

    pub async fn start_runtime(&self, runtime_id: &str) -> Result<(), RuntimeError> {
        let (config, status) = {
            let state = self.get_runtime(runtime_id).await?;
            (state.config.clone(), state.status)
        };

        if matches!(status, RuntimeStatus::Starting | RuntimeStatus::Ready) {
            return Ok(());
        }

        if config.runtime_type == RuntimeType::Blender {
            return Err(RuntimeError::HealthCheck(
                "Blender is on-demand, not a service".to_string(),
            ));
        }

        self.emit_init_log("info", &format!("Starting {} service", config.display_name), Some(runtime_id));

        self.update_runtime_state(runtime_id, |state| {
            state.status = RuntimeStatus::Starting;
            state.startup_timestamp = Some(Utc::now());
            state.error = None;
        })
        .await?;

        let launcher = config
            .launcher_path
            .as_deref()
            .or(config.install_path.as_deref())
            .ok_or_else(|| RuntimeError::ProcessSpawn("No launcher configured".to_string()))?;

        let launcher_path = PathBuf::from(launcher);
        if !launcher_path.exists() {
            let error_msg = format!("Launcher not found: {}", launcher_path.display());
            self.emit_init_log("error", &error_msg, Some(runtime_id));
            self.update_runtime_state(runtime_id, |state| {
                state.status = RuntimeStatus::Failed;
                state.error = Some(error_msg.clone());
            })
            .await?;
            return Err(RuntimeError::ProcessSpawn(error_msg));
        }

        self.emit_init_log("info", &format!("Launcher found: {}", launcher_path.display()), Some(runtime_id));

        let working_dir = config
            .working_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                launcher_path
                    .parent()
                    .unwrap_or(&PathBuf::from("."))
                    .to_path_buf()
            });

        let mut cmd = Command::new(&launcher_path);
        cmd.current_dir(&working_dir);
        cmd.args(&config.startup_args);
        cmd.envs(&config.environment);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(0x08000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::ProcessSpawn(e.to_string()))?;

        let pid = child.id();

        self.emit_init_log("info", &format!("Process started (PID: {})", pid), Some(runtime_id));

        {
            let mut processes = self.child_processes.lock().await;
            processes.insert(runtime_id.to_string(), child);
        }

        self.update_runtime_state(runtime_id, |state| {
            state.started_by_nexora = true;
            state.process_id = Some(pid);
        })
        .await?;

        let start_time = Instant::now();
        let timeout = Duration::from_secs(180);
        let poll_interval = Duration::from_secs(2);
        let mut poll_count = 0;

        while start_time.elapsed() < timeout {
            tokio::time::sleep(poll_interval).await;
            poll_count += 1;

            self.emit_init_log("info", &format!("Health check attempt {} ({}s elapsed)", poll_count, start_time.elapsed().as_secs()), Some(runtime_id));

            let health = self.check_health(runtime_id).await?;
            if health == RuntimeStatus::Ready {
                let readiness_ms = start_time.elapsed().as_millis() as u64;
                self.emit_init_log("info", &format!("{} ready ({}ms)", config.display_name, readiness_ms), Some(runtime_id));
                self.update_runtime_state(runtime_id, |state| {
                    state.readiness_time_ms = Some(readiness_ms);
                })
                .await?;
                return Ok(());
            }
        }

        let error_msg = format!("Timeout waiting for {} to become ready (180s)", config.display_name);
        self.emit_init_log("error", &error_msg, Some(runtime_id));
        self.update_runtime_state(runtime_id, |state| {
            state.status = RuntimeStatus::Failed;
            state.error = Some(error_msg);
        })
        .await?;

        self.stop_runtime(runtime_id).await?;
        Err(RuntimeError::Timeout)
    }

    pub async fn stop_runtime(&self, runtime_id: &str) -> Result<(), RuntimeError> {
        let (started_by_nexora, pid) = {
            let state = self.get_runtime(runtime_id).await?;
            (state.started_by_nexora, state.process_id)
        };

        if !started_by_nexora {
            return Err(RuntimeError::NotOwned);
        }

        let mut processes = self.child_processes.lock().await;
        if let Some(mut child) = processes.remove(runtime_id) {
            let _ = child.kill();
            let _ = child.wait();
        }

        self.update_runtime_state(runtime_id, |state| {
            state.status = RuntimeStatus::Stopped;
            state.started_by_nexora = false;
            state.process_id = None;
            state.startup_timestamp = None;
            state.readiness_time_ms = None;
        })
        .await?;

        Ok(())
    }

    pub async fn initialize_all(&self) -> Vec<(String, RuntimeStatus)> {
        self.emit_init_log("info", "Starting runtime initialization", None);

        let runtime_ids: Vec<String> = {
            let runtimes = self.runtimes.lock().await;
            runtimes.keys().cloned().collect()
        };

        self.emit_init_log("info", &format!("Found {} registered runtimes", runtime_ids.len()), None);

        let mut results = Vec::new();
        for runtime_id in runtime_ids {
            let config = {
                let state = self.get_runtime(&runtime_id).await.unwrap();
                state.config.clone()
            };

            self.emit_init_log("info", &format!("Processing {}", config.display_name), Some(&runtime_id));

            if !config.auto_start {
                self.emit_init_log("info", &format!("{} is on-demand, checking health", config.display_name), Some(&runtime_id));
                let status = self
                    .check_health(&runtime_id)
                    .await
                    .unwrap_or(RuntimeStatus::Failed);
                self.emit_init_log(
                    if status == RuntimeStatus::Ready { "info" } else { "warn" },
                    &format!("{} status: {:?}", config.display_name, status),
                    Some(&runtime_id),
                );
                results.push((runtime_id, status));
                continue;
            }

            self.emit_init_log("info", &format!("Checking if {} is already running", config.display_name), Some(&runtime_id));
            let health = self
                .check_health(&runtime_id)
                .await
                .unwrap_or(RuntimeStatus::Failed);

            if health == RuntimeStatus::Ready {
                self.emit_init_log("info", &format!("{} is already running", config.display_name), Some(&runtime_id));
                self.update_runtime_state(&runtime_id, |state| {
                    state.started_by_nexora = false;
                })
                .await
                .ok();
                results.push((runtime_id, RuntimeStatus::Ready));
            } else {
                self.emit_init_log("info", &format!("{} not running, starting...", config.display_name), Some(&runtime_id));
                let start_result = self.start_runtime(&runtime_id).await;
                let final_status = if start_result.is_ok() {
                    RuntimeStatus::Ready
                } else {
                    RuntimeStatus::Failed
                };
                self.emit_init_log(
                    if final_status == RuntimeStatus::Ready { "info" } else { "error" },
                    &format!("{} initialization: {:?}", config.display_name, final_status),
                    Some(&runtime_id),
                );
                results.push((runtime_id, final_status));
            }
        }

        let ready_count = results.iter().filter(|(_, s)| *s == RuntimeStatus::Ready).count();
        self.emit_init_log("info", &format!("Runtime initialization completed: {}/{} ready", ready_count, results.len()), None);
        results
    }

    pub async fn shutdown_all(&self) -> Result<(), RuntimeError> {
        let runtime_ids: Vec<String> = {
            let runtimes = self.runtimes.lock().await;
            runtimes
                .values()
                .filter(|s| s.started_by_nexora)
                .map(|s| s.config.runtime_id.clone())
                .collect()
        };

        for runtime_id in runtime_ids {
            let _ = self.stop_runtime(&runtime_id).await;
        }
        Ok(())
    }

    pub fn discover_a1111() -> Option<RuntimeConfig> {
        let base_path = PathBuf::from(r"C:\AI\stable-diffusion-webui");
        let venv_python = base_path.join("venv").join("Scripts").join("python.exe");
        let launch_py = base_path.join("launch.py");
        let launcher = base_path.join("webui-user.bat");

        if venv_python.exists() && launch_py.exists() {
            Some(RuntimeConfig {
                runtime_id: "automatic1111".to_string(),
                display_name: "Automatic1111".to_string(),
                runtime_type: RuntimeType::Automatic1111,
                kind: RuntimeKind::LongRunningService,
                install_path: Some(base_path.to_string_lossy().to_string()),
                launcher_path: Some(venv_python.to_string_lossy().to_string()),
                base_url: Some("http://127.0.0.1:7860".to_string()),
                health_endpoint: Some("/sdapi/v1/options".to_string()),
                auto_start: true,
                startup_args: vec![
                    "launch.py".to_string(),
                    "--api".to_string(),
                    "--medvram".to_string(),
                    "--nowebui".to_string(),
                    "--port".to_string(),
                    "7860".to_string(),
                ],
                working_directory: Some(base_path.to_string_lossy().to_string()),
                environment: HashMap::new(),
            })
        } else if launcher.exists() {
            Some(RuntimeConfig {
                runtime_id: "automatic1111".to_string(),
                display_name: "Automatic1111".to_string(),
                runtime_type: RuntimeType::Automatic1111,
                kind: RuntimeKind::LongRunningService,
                install_path: Some(base_path.to_string_lossy().to_string()),
                launcher_path: Some(launcher.to_string_lossy().to_string()),
                base_url: Some("http://127.0.0.1:7860".to_string()),
                health_endpoint: Some("/sdapi/v1/options".to_string()),
                auto_start: true,
                startup_args: vec!["--api".to_string(), "--medvram".to_string()],
                working_directory: Some(base_path.to_string_lossy().to_string()),
                environment: HashMap::new(),
            })
        } else {
            None
        }
    }

    pub fn discover_hunyuan3d() -> Option<RuntimeConfig> {
        let base_path = PathBuf::from(r"C:\AI\Hunyuan3D");
        let entrypoint = base_path.join("api_server.py");

        if entrypoint.exists() {
            let python = find_hunyuan_python(&base_path)?;
            Some(RuntimeConfig {
                runtime_id: "hunyuan3d".to_string(),
                display_name: "Hunyuan3D".to_string(),
                runtime_type: RuntimeType::Hunyuan3D,
                kind: RuntimeKind::LongRunningService,
                install_path: Some(base_path.to_string_lossy().to_string()),
                launcher_path: Some(python.to_string_lossy().to_string()),
                base_url: Some("http://127.0.0.1:8081".to_string()),
                health_endpoint: Some("/health".to_string()),
                auto_start: true,
                startup_args: vec![
                    entrypoint.to_string_lossy().to_string(),
                    "--host".to_string(),
                    "127.0.0.1".to_string(),
                    "--port".to_string(),
                    "8081".to_string(),
                    "--limit-model-concurrency".to_string(),
                    "1".to_string(),
                ],
                working_directory: Some(base_path.to_string_lossy().to_string()),
                environment: HashMap::new(),
            })
        } else {
            None
        }
    }

    pub fn discover_blender() -> Option<RuntimeConfig> {
        let executable =
            PathBuf::from(r"C:\Program Files\Blender Foundation\Blender 5.2\blender.exe");

        if executable.exists() {
            Some(RuntimeConfig {
                runtime_id: "blender".to_string(),
                display_name: "Blender".to_string(),
                runtime_type: RuntimeType::Blender,
                kind: RuntimeKind::OnDemandExecutable,
                install_path: Some(executable.parent().unwrap().to_string_lossy().to_string()),
                launcher_path: Some(executable.to_string_lossy().to_string()),
                base_url: None,
                health_endpoint: None,
                auto_start: false,
                startup_args: vec![],
                working_directory: None,
                environment: HashMap::new(),
            })
        } else {
            None
        }
    }
}

fn find_hunyuan_python(base_path: &Path) -> Option<PathBuf> {
    let venv_python = base_path.join(".venv").join("Scripts").join("python.exe");
    if venv_python.exists() {
        return Some(venv_python);
    }
    let venv_python = base_path.join("venv").join("Scripts").join("python.exe");
    if venv_python.exists() {
        return Some(venv_python);
    }
    let python = which::which("python").ok();
    python
}

impl Default for RuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_default() {
        let config = RuntimeConfig::default();
        assert_eq!(config.runtime_id, "");
        assert_eq!(config.auto_start, false);
    }

    #[test]
    fn test_runtime_state_new() {
        let config = RuntimeConfig {
            runtime_id: "test".to_string(),
            display_name: "Test".to_string(),
            runtime_type: RuntimeType::Automatic1111,
            kind: RuntimeKind::LongRunningService,
            auto_start: true,
            ..Default::default()
        };
        let state = RuntimeState::new(config);
        assert_eq!(state.status, RuntimeStatus::NotConfigured);
        assert!(!state.started_by_nexora);
        assert!(state.process_id.is_none());
    }

    #[test]
    fn test_discover_a1111() {
        let config = RuntimeManager::discover_a1111();
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.runtime_id, "automatic1111");
        assert_eq!(config.base_url, Some("http://127.0.0.1:7860".to_string()));
    }

    #[test]
    fn test_discover_hunyuan3d() {
        let config = RuntimeManager::discover_hunyuan3d();
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.runtime_id, "hunyuan3d");
        assert_eq!(config.base_url, Some("http://127.0.0.1:8081".to_string()));
        assert!(
            config
                .startup_args
                .iter()
                .any(|a| a.contains("limit-model-concurrency"))
        );
    }

    #[test]
    fn test_discover_blender() {
        let config = RuntimeManager::discover_blender();
        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.runtime_id, "blender");
        assert_eq!(config.kind, RuntimeKind::OnDemandExecutable);
        assert!(!config.auto_start);
    }
}

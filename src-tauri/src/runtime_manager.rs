use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use which::which;

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
        f(state);
        Ok(state.clone())
    }

    pub async fn check_health(&self, runtime_id: &str) -> Result<RuntimeStatus, RuntimeError> {
        let config = {
            let state = self.get_runtime(runtime_id).await?;
            state.config
        };

        let status = match config.runtime_type {
            RuntimeType::Automatic1111 => self.check_a1111_health(&config).await,
            RuntimeType::Hunyuan3D => self.check_hunyuan_health(&config).await,
            RuntimeType::Blender => self.check_blender_health(&config).await,
        };

        let (new_status, error_str) = match &status {
            Ok(()) => (RuntimeStatus::Ready, None),
            Err(e) => {
                let err_str = e.to_string();
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;

        let response = client.get(format!("{}/docs", base_url)).send().await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(RuntimeError::HealthCheck(format!(
                "Hunyuan3D returned {}",
                response.status()
            )))
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

        let output = Command::new(executable).arg("--version").output()?;

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
            self.update_runtime_state(runtime_id, |state| {
                state.status = RuntimeStatus::Failed;
                state.error = Some(format!("Launcher not found: {}", launcher_path.display()));
            })
            .await?;
            return Err(RuntimeError::ProcessSpawn(format!(
                "Launcher not found: {}",
                launcher_path.display()
            )));
        }

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

        let mut child = cmd
            .spawn()
            .map_err(|e| RuntimeError::ProcessSpawn(e.to_string()))?;

        let pid = child.id();

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

        while start_time.elapsed() < timeout {
            tokio::time::sleep(poll_interval).await;

            let health = self.check_health(runtime_id).await?;
            if health == RuntimeStatus::Ready {
                self.update_runtime_state(runtime_id, |state| {
                    state.readiness_time_ms = Some(start_time.elapsed().as_millis() as u64);
                })
                .await?;
                return Ok(());
            }
        }

        self.update_runtime_state(runtime_id, |state| {
            state.status = RuntimeStatus::Failed;
            state.error = Some("Timeout waiting for service to become ready".to_string());
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
        let runtime_ids: Vec<String> = {
            let runtimes = self.runtimes.lock().await;
            runtimes.keys().cloned().collect()
        };

        let mut results = Vec::new();
        for runtime_id in runtime_ids {
            let config = {
                let state = self.get_runtime(&runtime_id).await.unwrap();
                state.config.clone()
            };

            if !config.auto_start {
                let status = self
                    .check_health(&runtime_id)
                    .await
                    .unwrap_or(RuntimeStatus::Failed);
                results.push((runtime_id, status));
                continue;
            }

            let health = self
                .check_health(&runtime_id)
                .await
                .unwrap_or(RuntimeStatus::Failed);

            if health == RuntimeStatus::Ready {
                self.update_runtime_state(&runtime_id, |state| {
                    state.started_by_nexora = false;
                })
                .await
                .ok();
                results.push((runtime_id, RuntimeStatus::Ready));
            } else {
                let start_result = self.start_runtime(&runtime_id).await;
                let final_status = if start_result.is_ok() {
                    RuntimeStatus::Ready
                } else {
                    RuntimeStatus::Failed
                };
                results.push((runtime_id, final_status));
            }
        }

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
        let launcher = base_path.join("webui-user.bat");

        if launcher.exists() {
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
                health_endpoint: Some("/docs".to_string()),
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

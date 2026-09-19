use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::Emitter;
use tokio::sync::Mutex as AsyncMutex;

use crate::runtime_manager::{RuntimeManager, RuntimeStatus};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    Idle,
    Queued,
    Starting,
    Initializing,
    HealthChecking,
    Ready,
    Degraded,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub provider_id: String,
    pub display_name: String,
    pub state: ProviderState,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub ready_at: Option<String>,
    pub health_check_count: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestratorStatus {
    pub state: ProviderState,
    pub current_provider: Option<String>,
    pub providers: Vec<ProviderInfo>,
    pub startup_order: Vec<String>,
    pub completed_count: usize,
    pub total_count: usize,
}

#[derive(Clone)]
pub struct ProviderOrchestrator {
    runtime_manager: Arc<RuntimeManager>,
    app_handle: Option<tauri::AppHandle>,
    startup_order: Vec<String>,
    provider_states: Arc<AsyncMutex<HashMap<String, ProviderState>>>,
    provider_errors: Arc<AsyncMutex<HashMap<String, String>>>,
    provider_health_counts: Arc<AsyncMutex<HashMap<String, u32>>>,
    is_initialized: Arc<AsyncMutex<bool>>,
}

impl ProviderOrchestrator {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            runtime_manager,
            app_handle: None,
            startup_order: vec![
                "automatic1111".to_string(),
                "hunyuan3d".to_string(),
                "blender".to_string(),
            ],
            provider_states: Arc::new(AsyncMutex::new(HashMap::new())),
            provider_errors: Arc::new(AsyncMutex::new(HashMap::new())),
            provider_health_counts: Arc::new(AsyncMutex::new(HashMap::new())),
            is_initialized: Arc::new(AsyncMutex::new(false)),
        }
    }

    pub fn set_app_handle(&mut self, handle: tauri::AppHandle) {
        self.app_handle = Some(handle);
    }

    pub fn set_startup_order(&mut self, order: Vec<String>) {
        self.startup_order = order;
    }

    fn emit_event(&self, event: &str, payload: impl Serialize + Clone) {
        if let Some(handle) = &self.app_handle {
            let _ = handle.emit(event, payload);
        }
    }

    fn emit_log(&self, level: &str, provider_id: &str, message: &str) {
        self.emit_event(
            "orchestrator://log",
            serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "level": level,
                "providerId": provider_id,
                "message": message,
            }),
        );
    }

    fn emit_state_change(&self, provider_id: &str, state: ProviderState) {
        self.emit_event(
            "orchestrator://state-changed",
            serde_json::json!({
                "providerId": provider_id,
                "state": state,
            }),
        );
    }

    fn emit_progress(&self) {
        let states = self.provider_states.clone();
        let errors = self.provider_errors.clone();
        let health_counts = self.provider_health_counts.clone();
        let startup_order = self.startup_order.clone();
        let app_handle = self.app_handle.clone();

        tokio::spawn(async move {
            let states = states.lock().await;
            let errors = errors.lock().await;
            let health_counts = health_counts.lock().await;

            let providers: Vec<ProviderInfo> = startup_order
                .iter()
                .map(|id| {
                    let state = states.get(id).copied().unwrap_or(ProviderState::Idle);
                    let error = errors.get(id).cloned();
                    let health_check_count = health_counts.get(id).copied().unwrap_or(0);

                    ProviderInfo {
                        provider_id: id.clone(),
                        display_name: get_display_name(id),
                        state,
                        error,
                        started_at: None,
                        ready_at: None,
                        health_check_count,
                    }
                })
                .collect();

            let completed_count = providers
                .iter()
                .filter(|p| matches!(p.state, ProviderState::Ready | ProviderState::Degraded))
                .count();

            let current_provider = providers
                .iter()
                .find(|p| {
                    matches!(
                        p.state,
                        ProviderState::Starting
                            | ProviderState::Initializing
                            | ProviderState::HealthChecking
                    )
                })
                .map(|p| p.provider_id.clone());

            let status = OrchestratorStatus {
                state: if completed_count == startup_order.len() {
                    ProviderState::Ready
                } else if providers.iter().any(|p| p.state == ProviderState::Starting) {
                    ProviderState::Starting
                } else {
                    ProviderState::Idle
                },
                current_provider,
                providers,
                startup_order: startup_order.clone(),
                completed_count,
                total_count: startup_order.len(),
            };

            if let Some(handle) = app_handle {
                let _ = handle.emit("orchestrator://progress", status);
            }
        });
    }

    pub async fn initialize_sequential(&self) -> Result<(), String> {
        let mut is_initialized = self.is_initialized.lock().await;
        if *is_initialized {
            return Ok(());
        }
        *is_initialized = true;
        drop(is_initialized);

        self.emit_log("info", "system", "Starting sequential provider initialization");
        self.emit_event(
            "orchestrator://started",
            serde_json::json!({
                "startupOrder": self.startup_order,
            }),
        );

        for provider_id in &self.startup_order {
            self.initialize_provider(provider_id).await;
        }

        let states = self.provider_states.lock().await;
        let ready_count = states
            .values()
            .filter(|s| matches!(s, ProviderState::Ready | ProviderState::Degraded))
            .count();

        self.emit_log(
            "info",
            "system",
            &format!(
                "Provider initialization completed: {}/{} ready",
                ready_count,
                self.startup_order.len()
            ),
        );

        self.emit_event(
            "orchestrator://completed",
            serde_json::json!({
                "readyCount": ready_count,
                "totalCount": self.startup_order.len(),
            }),
        );

        Ok(())
    }

    async fn initialize_provider(&self, provider_id: &str) {
        let display_name = get_display_name(provider_id);

        self.emit_log("info", provider_id, &format!("Initializing {}", display_name));
        self.set_provider_state(provider_id, ProviderState::Queued).await;
        self.emit_progress();

        // Check if already running
        self.emit_log("info", provider_id, &format!("Checking if {} is already running", display_name));
        self.set_provider_state(provider_id, ProviderState::HealthChecking).await;
        self.emit_progress();

        let health = self
            .runtime_manager
            .check_health(provider_id)
            .await
            .unwrap_or(RuntimeStatus::Failed);

        if health == RuntimeStatus::Ready {
            self.emit_log("info", provider_id, &format!("{} is already running", display_name));
            self.set_provider_state(provider_id, ProviderState::Ready).await;
            self.emit_progress();
            return;
        }

        // Start the provider
        self.emit_log("info", provider_id, &format!("Starting {}", display_name));
        self.set_provider_state(provider_id, ProviderState::Starting).await;
        self.emit_progress();

        let start_result = self.runtime_manager.start_runtime(provider_id).await;

        if start_result.is_err() {
            let error = start_result.unwrap_err().to_string();
            self.emit_log("error", provider_id, &format!("Failed to start {}: {}", display_name, error));
            self.set_provider_error(provider_id, error).await;
            self.set_provider_state(provider_id, ProviderState::Failed).await;
            self.emit_progress();
            return;
        }

        // Wait for ready with health checks
        self.emit_log("info", provider_id, &format!("Waiting for {} to become ready", display_name));
        self.set_provider_state(provider_id, ProviderState::Initializing).await;
        self.emit_progress();

        let timeout = Duration::from_secs(180);
        let poll_interval = Duration::from_secs(2);
        let start_time = Instant::now();
        let mut poll_count = 0;

        while start_time.elapsed() < timeout {
            tokio::time::sleep(poll_interval).await;
            poll_count += 1;

            self.increment_health_count(provider_id).await;
            self.emit_log(
                "info",
                provider_id,
                &format!(
                    "Health check {}/{} ({}s elapsed)",
                    poll_count,
                    timeout.as_secs() / poll_interval.as_secs(),
                    start_time.elapsed().as_secs()
                ),
            );
            self.emit_progress();

            let health = self
                .runtime_manager
                .check_health(provider_id)
                .await
                .unwrap_or(RuntimeStatus::Failed);

            if health == RuntimeStatus::Ready {
                self.emit_log("info", provider_id, &format!("{} is ready", display_name));
                self.set_provider_state(provider_id, ProviderState::Ready).await;
                self.emit_progress();
                return;
            }
        }

        // Timeout
        let error = format!("Timeout waiting for {} to become ready (180s)", display_name);
        self.emit_log("error", provider_id, &error);
        self.set_provider_error(provider_id, error).await;
        self.set_provider_state(provider_id, ProviderState::Failed).await;
        self.emit_progress();
    }

    async fn set_provider_state(&self, provider_id: &str, state: ProviderState) {
        let mut states = self.provider_states.lock().await;
        states.insert(provider_id.to_string(), state);
        self.emit_state_change(provider_id, state);
    }

    async fn set_provider_error(&self, provider_id: &str, error: String) {
        let mut errors = self.provider_errors.lock().await;
        errors.insert(provider_id.to_string(), error);
    }

    async fn increment_health_count(&self, provider_id: &str) {
        let mut counts = self.provider_health_counts.lock().await;
        let count = counts.entry(provider_id.to_string()).or_insert(0);
        *count += 1;
    }

    pub async fn get_status(&self) -> OrchestratorStatus {
        let states = self.provider_states.lock().await;
        let errors = self.provider_errors.lock().await;
        let health_counts = self.provider_health_counts.lock().await;

        let providers: Vec<ProviderInfo> = self
            .startup_order
            .iter()
            .map(|id| {
                let state = states.get(id).copied().unwrap_or(ProviderState::Idle);
                let error = errors.get(id).cloned();
                let health_check_count = health_counts.get(id).copied().unwrap_or(0);

                ProviderInfo {
                    provider_id: id.clone(),
                    display_name: get_display_name(id),
                    state,
                    error,
                    started_at: None,
                    ready_at: None,
                    health_check_count,
                }
            })
            .collect();

        let completed_count = providers
            .iter()
            .filter(|p| matches!(p.state, ProviderState::Ready | ProviderState::Degraded))
            .count();

        let current_provider = providers
            .iter()
            .find(|p| {
                matches!(
                    p.state,
                    ProviderState::Starting
                        | ProviderState::Initializing
                        | ProviderState::HealthChecking
                )
            })
            .map(|p| p.provider_id.clone());

        OrchestratorStatus {
            state: if completed_count == self.startup_order.len() {
                ProviderState::Ready
            } else if providers.iter().any(|p| p.state == ProviderState::Starting) {
                ProviderState::Starting
            } else {
                ProviderState::Idle
            },
            current_provider,
            providers,
            startup_order: self.startup_order.clone(),
            completed_count,
            total_count: self.startup_order.len(),
        }
    }

    pub async fn shutdown_all(&self) -> Result<(), String> {
        self.emit_log("info", "system", "Shutting down all providers");

        for provider_id in self.startup_order.iter().rev() {
            let display_name = get_display_name(provider_id);
            self.emit_log("info", provider_id, &format!("Stopping {}", display_name));
            self.set_provider_state(provider_id, ProviderState::Stopping).await;

            let _ = self.runtime_manager.stop_runtime(provider_id).await;
            self.set_provider_state(provider_id, ProviderState::Stopped).await;
        }

        self.emit_log("info", "system", "All providers stopped");
        Ok(())
    }
}

fn get_display_name(provider_id: &str) -> String {
    match provider_id {
        "automatic1111" => "Automatic1111".to_string(),
        "hunyuan3d" => "Hunyuan3D".to_string(),
        "blender" => "Blender".to_string(),
        _ => provider_id.to_string(),
    }
}

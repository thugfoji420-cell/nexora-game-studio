use crate::providers::{Capability, Classification, ExecutionMode, HealthState, HardwareRequirements, LicenseMetadata, ProviderManifest, ProviderNature, ProviderType, ResourceClass};
use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

pub const PROVIDER_ID: &str = "remote.openrouter";
pub const PROVIDER_VERSION: &str = "1.0.0";
const CONFIG_SCHEMA_VERSION: u32 = 1;
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_MODEL_LIST_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenRouterConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub provider_id: String,
    pub base_url: String,
    /// The raw API key. This is sensitive: never serialized to the frontend.
    /// `save_settings` from the IPC boundary always overwrites this field with
    /// the in-memory value because the frontend only sees the masked shape.
    #[serde(default)]
    pub api_key: String,
    pub referer: String,
    pub title: String,
    pub default_model: String,
    pub timeout_seconds: u64,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            enabled: false,
            provider_id: PROVIDER_ID.into(),
            base_url: DEFAULT_BASE_URL.into(),
            api_key: String::new(),
            referer: String::new(),
            title: String::new(),
            default_model: String::new(),
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpenRouterError {
    #[error("invalid openrouter config: {0}")]
    InvalidConfig(String),
    #[error("api key is required")]
    MissingApiKey,
    #[error("default model is required")]
    MissingModel,
    #[error("invalid api key")]
    InvalidApiKey,
    #[error("rate limited")]
    RateLimited,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("timeout")]
    Timeout,
    #[error("invalid json: {0}")]
    InvalidJson(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("empty response")]
    EmptyResponse,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
}

impl OpenRouterError {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Network(_) | Self::Timeout | Self::Provider(_))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    pub provider: Option<String>,
    pub context_length: Option<u32>,
    pub pricing: Option<OpenRouterPricing>,
    pub top_provider: Option<OpenRouterTopProvider>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterPricing {
    pub prompt: Option<String>,
    pub completion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterTopProvider {
    pub context_length: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub is_moderated: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterChatRequest {
    pub model: String,
    pub messages: Vec<OpenRouterChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterChatChoice {
    pub message: OpenRouterChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterChatUsage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<OpenRouterChatChoice>,
    pub usage: Option<OpenRouterChatUsage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterModelsResponse {
    pub data: Vec<OpenRouterModel>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterErrorResponse {
    pub error: OpenRouterErrorDetail,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterErrorDetail {
    pub message: String,
    pub code: Option<i32>,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterHealth {
    pub provider_id: String,
    pub state: HealthState,
    pub checked_at: DateTime<Utc>,
    pub detail: Option<String>,
}

impl OpenRouterConfig {
    pub fn validate(&self) -> Result<(), OpenRouterError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(OpenRouterError::InvalidConfig(
                "unsupported schema version".into(),
            ));
        }
        if self.provider_id != PROVIDER_ID {
            return Err(OpenRouterError::InvalidConfig(
                "invalid provider id".into(),
            ));
        }
        if self.base_url.is_empty() {
            return Err(OpenRouterError::InvalidConfig(
                "base_url is required".into(),
            ));
        }
        let _ = reqwest::Url::parse(&self.base_url).map_err(|_| {
            OpenRouterError::InvalidConfig("base_url is not a valid url".into())
        })?;
        if self.timeout_seconds == 0 || self.timeout_seconds > 600 {
            return Err(OpenRouterError::InvalidConfig(
                "timeout_seconds must be 1..600".into(),
            ));
        }
        Ok(())
    }

    pub fn masked(&self) -> MaskedOpenRouterConfig {
        MaskedOpenRouterConfig {
            schema_version: self.schema_version,
            enabled: self.enabled,
            provider_id: self.provider_id.clone(),
            base_url: self.base_url.clone(),
            api_key_masked: mask_secret(&self.api_key),
            api_key_configured: !self.api_key.is_empty(),
            referer: self.referer.clone(),
            title: self.title.clone(),
            default_model: self.default_model.clone(),
            timeout_seconds: self.timeout_seconds,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskedOpenRouterConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub provider_id: String,
    pub base_url: String,
    pub api_key_masked: String,
    pub api_key_configured: bool,
    pub referer: String,
    pub title: String,
    pub default_model: String,
    pub timeout_seconds: u64,
}

pub fn provider_manifest() -> ProviderManifest {
    ProviderManifest {
        schema_version: crate::providers::PROVIDER_MANIFEST_SCHEMA_VERSION,
        provider_id: PROVIDER_ID.into(),
        display_name: "OpenRouter".into(),
        version: PROVIDER_VERSION.into(),
        provider_type: ProviderType::Utility,
        execution_mode: ExecutionMode::RemoteApi,
        nature: ProviderNature::Real,
        classification: Classification::Remote,
        enabled: false,
        capabilities: vec![Capability::PromptGeneration],
        health_check: crate::providers::HealthCheckType::Manual,
        requirements: requirements(ResourceClass::Minimal),
        license: LicenseMetadata {
            status: crate::providers::LicenseStatus::Known,
            name: Some("OpenRouter".into()),
            model_license: Some("varies by model".into()),
            commercial_use_allowed: Some(true),
            source_reference: Some("https://openrouter.ai/models".into()),
        },
        permissions: vec![crate::providers::Permission::Network],
    }
}

pub fn health(config: &OpenRouterConfig) -> OpenRouterHealth {
    if !config.enabled {
        return OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Unavailable,
            checked_at: Utc::now(),
            detail: Some("provider is disabled".into()),
        };
    }
    if config.api_key.is_empty() {
        return OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Misconfigured,
            checked_at: Utc::now(),
            detail: Some("api key is not configured".into()),
        };
    }
    if config.default_model.is_empty() {
        return OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Misconfigured,
            checked_at: Utc::now(),
            detail: Some("default model is not configured".into()),
        };
    }
    match test_connection_internal(config) {
        Ok(_) => OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Healthy,
            checked_at: Utc::now(),
            detail: Some("connected".into()),
        },
        Err(OpenRouterError::InvalidApiKey | OpenRouterError::Unauthorized | OpenRouterError::Forbidden) => OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Misconfigured,
            checked_at: Utc::now(),
            detail: Some("invalid api key".into()),
        },
        Err(OpenRouterError::RateLimited) => OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Degraded,
            checked_at: Utc::now(),
            detail: Some("rate limited".into()),
        },
        Err(error) => OpenRouterHealth {
            provider_id: PROVIDER_ID.into(),
            state: HealthState::Unavailable,
            checked_at: Utc::now(),
            detail: Some(error.to_string()),
        },
    }
}

pub fn test_connection(config: &OpenRouterConfig) -> Result<(), OpenRouterError> {
    validate_configured(config)?;
    test_connection_internal(config)
}

pub fn list_models(config: &OpenRouterConfig) -> Result<Vec<OpenRouterModel>, OpenRouterError> {
    validate_configured(config)?;
    let client = build_client(config)?;
    let url = build_url(config, "/models")?;
    let response = client
        .get(url)
        .send()
        .map_err(|e| classify_transport(e, "list_models"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(OpenRouterError::InvalidApiKey);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(OpenRouterError::RateLimited);
    }
    if !status.is_success() {
        return Err(OpenRouterError::Provider(format!(
            "openrouter models endpoint returned {}",
            status
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|e| OpenRouterError::Network(e.to_string()))?;
    if bytes.is_empty() {
        return Err(OpenRouterError::EmptyResponse);
    }
    let parsed: OpenRouterModelsResponse =
        serde_json::from_slice(bytes.as_ref()).map_err(|e| OpenRouterError::InvalidJson(e.to_string()))?;
    Ok(parsed.data)
}

pub fn chat_completion(
    config: &OpenRouterConfig,
    model_id: &str,
    system_prompt: Option<&str>,
    user_prompt: &str,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Result<OpenRouterChatResponse, OpenRouterError> {
    if model_id.trim().is_empty() {
        return Err(OpenRouterError::ModelNotFound(
            "model id is empty".into(),
        ));
    }
    if user_prompt.trim().is_empty() {
        return Err(OpenRouterError::InvalidConfig(
            "user prompt is empty".into(),
        ));
    }
    validate_configured(config)?;
    let client = build_client(config)?;
    let url = build_url(config, "/chat/completions")?;
    let mut messages = Vec::new();
    if let Some(system) = system_prompt {
        if !system.trim().is_empty() {
            messages.push(OpenRouterChatMessage {
                role: "system".into(),
                content: system.trim().into(),
            });
        }
    }
    messages.push(OpenRouterChatMessage {
        role: "user".into(),
        content: user_prompt.trim().into(),
    });
    let body = OpenRouterChatRequest {
        model: model_id.trim().into(),
        messages,
        temperature: temperature.and_then(|t| {
            if t.is_finite() && (0.0..=2.0).contains(&t) {
                Some(t)
            } else {
                None
            }
        }),
        max_tokens,
    };
    let response = client
        .post(url)
        .json(&body)
        .send()
        .map_err(|e| classify_transport(e, "chat_completion"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(OpenRouterError::InvalidApiKey);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(OpenRouterError::RateLimited);
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(OpenRouterError::ModelNotFound(model_id.into()));
    }
    if status == reqwest::StatusCode::BAD_REQUEST {
        let text = response.text().unwrap_or_default();
        return Err(OpenRouterError::Provider(format!(
            "bad request: {}",
            truncate(&text, 200)
        )));
    }
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(OpenRouterError::Provider(format!(
            "openrouter chat endpoint returned {}: {}",
            status,
            truncate(&text, 200)
        )));
    }
    let bytes = response
        .bytes()
        .map_err(|e| OpenRouterError::Network(e.to_string()))?;
    if bytes.is_empty() {
        return Err(OpenRouterError::EmptyResponse);
    }
    let parsed: OpenRouterChatResponse =
        serde_json::from_slice(bytes.as_ref()).map_err(|e| OpenRouterError::InvalidJson(e.to_string()))?;
    if parsed.choices.is_empty() {
        return Err(OpenRouterError::EmptyResponse);
    }
    Ok(parsed)
}

pub fn is_free_model(model: &OpenRouterModel) -> bool {
    match &model.pricing {
        Some(pricing) => {
            let prompt_free = pricing
                .prompt
                .as_deref()
                .map(|v| v == "0" || v == "0.0")
                .unwrap_or(false);
            let completion_free = pricing
                .completion
                .as_deref()
                .map(|v| v == "0" || v == "0.0")
                .unwrap_or(false);
            prompt_free && completion_free
        }
        None => false,
    }
}

fn validate_configured(config: &OpenRouterConfig) -> Result<(), OpenRouterError> {
    config.validate()?;
    if config.api_key.is_empty() {
        return Err(OpenRouterError::MissingApiKey);
    }
    Ok(())
}

fn test_connection_internal(config: &OpenRouterConfig) -> Result<(), OpenRouterError> {
    let client = build_client(config)?;
    let url = build_url(config, "/models")?;
    let response = client
        .get(url)
        .send()
        .map_err(|e| classify_transport(e, "test_connection"))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(OpenRouterError::InvalidApiKey);
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(OpenRouterError::RateLimited);
    }
    if !status.is_success() {
        return Err(OpenRouterError::Provider(format!(
            "openrouter health check returned {}",
            status
        )));
    }
    Ok(())
}

fn build_client(config: &OpenRouterConfig) -> Result<Client, OpenRouterError> {
    let mut headers = reqwest::header::HeaderMap::new();
    if !config.referer.is_empty() {
        headers.insert(
            reqwest::header::HeaderName::from_static("http-referer"),
            config.referer.parse().map_err(|_| {
                OpenRouterError::InvalidConfig("referer is not a valid header value".into())
            })?,
        );
    }
    if !config.title.is_empty() {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-title"),
            config.title.parse().map_err(|_| {
                OpenRouterError::InvalidConfig("title is not a valid header value".into())
            })?,
        );
    }
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", config.api_key).parse().map_err(|_| {
            OpenRouterError::InvalidConfig("api key is not a valid header value".into())
        })?,
    );
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(config.timeout_seconds.min(30)))
        .timeout(Duration::from_secs(config.timeout_seconds))
        .default_headers(headers)
        .build()
        .map_err(|e| OpenRouterError::Network(e.to_string()))
}

fn build_url(config: &OpenRouterConfig, path: &str) -> Result<reqwest::Url, OpenRouterError> {
    let base = reqwest::Url::parse(&config.base_url).map_err(|_| {
        OpenRouterError::InvalidConfig("base_url is not a valid url".into())
    })?;
    let mut url = base;
    url.set_path(path);
    Ok(url)
}

fn classify_transport(error: reqwest::Error, context: &str) -> OpenRouterError {
    if error.is_timeout() {
        OpenRouterError::Timeout
    } else if error.is_connect() {
        OpenRouterError::Network(format!("{}: connection failed", context))
    } else {
        OpenRouterError::Network(format!("{}: {}", context, error))
    }
}

fn mask_secret(secret: &str) -> String {
    if secret.is_empty() {
        return "••••".into();
    }
    let len = secret.len();
    if len <= 8 {
        "••••••••".into()
    } else {
        let prefix = &secret[..4];
        let suffix = &secret[len - 4..];
        format!("{}••••{}", prefix, suffix)
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.into()
    } else {
        format!("{}...", &text[..max])
    }
}

fn requirements(resource_class: ResourceClass) -> HardwareRequirements {
    HardwareRequirements {
        min_ram_mib: None,
        recommended_ram_mib: None,
        min_vram_mib: None,
        recommended_vram_mib: None,
        gpu_required: false,
        supported_gpu_vendors: Vec::new(),
        cpu_fallback: false,
        min_disk_mib: None,
        exclusive: false,
        resource_class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_safe_values() {
        let config = OpenRouterConfig::default();
        assert!(!config.enabled);
        assert!(config.api_key.is_empty());
        assert!(config.default_model.is_empty());
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    }

    #[test]
    fn masked_config_never_exposes_full_key() {
        let mut config = OpenRouterConfig::default();
        config.api_key = "sk-or-verysecretkey123".into();
        let masked = config.masked();
        assert!(!masked.api_key_masked.contains("verysecretkey123"));
        assert!(masked.api_key_configured);
        assert_eq!(masked.api_key_masked, "sk-o••••y123");
    }

    #[test]
    fn empty_key_is_masked_as_dots() {
        let config = OpenRouterConfig::default();
        let masked = config.masked();
        assert_eq!(masked.api_key_masked, "••••");
        assert!(!masked.api_key_configured);
    }

    #[test]
    fn short_key_is_masked_fully() {
        let mut config = OpenRouterConfig::default();
        config.api_key = "short".into();
        let masked = config.masked();
        assert_eq!(masked.api_key_masked, "••••••••");
    }

    #[test]
    fn validate_rejects_bad_schema_and_url() {
        let mut config = OpenRouterConfig::default();
        config.schema_version = 99;
        assert!(config.validate().is_err());
        config.schema_version = CONFIG_SCHEMA_VERSION;
        config.base_url = "not-a-url".into();
        assert!(config.validate().is_err());
        config.base_url = DEFAULT_BASE_URL.into();
        config.timeout_seconds = 0;
        assert!(config.validate().is_err());
        config.timeout_seconds = 601;
        assert!(config.validate().is_err());
    }

    #[test]
    fn free_model_detection() {
        let free = OpenRouterModel {
            id: "some/free-model".into(),
            name: "Free Model".into(),
            provider: None,
            context_length: Some(4096),
            pricing: Some(OpenRouterPricing {
                prompt: Some("0".into()),
                completion: Some("0".into()),
            }),
            top_provider: None,
        };
        assert!(is_free_model(&free));

        let paid = OpenRouterModel {
            id: "some/paid-model".into(),
            name: "Paid Model".into(),
            provider: None,
            context_length: Some(4096),
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.001".into()),
                completion: Some("0.002".into()),
            }),
            top_provider: None,
        };
        assert!(!is_free_model(&paid));

        let missing_pricing = OpenRouterModel {
            id: "some/unknown-model".into(),
            name: "Unknown".into(),
            provider: None,
            context_length: None,
            pricing: None,
            top_provider: None,
        };
        assert!(!is_free_model(&missing_pricing));
    }

    #[test]
    fn model_response_parsing() {
        let json = r#"{"data":[{"id":"meta-llama/llama-3.1-8b-instruct","name":"Llama 3.1 8B","provider":"Meta","context_length":8192,"pricing":{"prompt":"0","completion":"0"},"top_provider":{"context_length":8192}}]}"#;
        let parsed: OpenRouterModelsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].id, "meta-llama/llama-3.1-8b-instruct");
        assert!(is_free_model(&parsed.data[0]));
    }

    #[test]
    fn error_mapping_does_not_leak_key() {
        let err = OpenRouterError::InvalidApiKey;
        assert_eq!(err.to_string(), "invalid api key");
        let err = OpenRouterError::Unauthorized;
        assert_eq!(err.to_string(), "unauthorized");
        let err = OpenRouterError::Forbidden;
        assert_eq!(err.to_string(), "forbidden");
        let err = OpenRouterError::MissingApiKey;
        assert_eq!(err.to_string(), "api key is required");
    }
}

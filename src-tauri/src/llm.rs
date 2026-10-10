//! Unified LLM client supporting multiple AI providers.
//!
//! Provides a single `chat` interface that dispatches to the configured
//! provider (OpenAI, Anthropic, Google Gemini, or any OpenAI-compatible
//! endpoint — Nvidia NIM, LM Studio, Ollama, vLLM, custom servers) and
//! returns a unified response. This is the app's own AI brain: no external
//! aggregator service is required.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Provider definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    OpenAI,
    Anthropic,
    Gemini,
    /// Any OpenAI-compatible /chat/completions endpoint: Nvidia NIM,
    /// LM Studio, Ollama, vLLM, or a self-hosted server.
    Custom,
    /// Fully offline local runtime: Ollama / llama.cpp `llama-server` /
    /// LM Studio on this machine. No API key, no internet — the app can
    /// chat and self-code with zero cloud dependencies.
    Local,
}

impl LlmProvider {
    pub fn display_name(&self) -> &'static str {
        match self {
            LlmProvider::OpenAI => "OpenAI",
            LlmProvider::Anthropic => "Anthropic",
            LlmProvider::Gemini => "Google Gemini",
            LlmProvider::Custom => "Custom (OpenAI-compatible)",
            LlmProvider::Local => "Offline local model",
        }
    }

    pub fn default_model(&self) -> &'static str {
        match self {
            LlmProvider::OpenAI => "gpt-5.6-terra",
            LlmProvider::Anthropic => "claude-opus-5-5",
            LlmProvider::Gemini => "gemini-3.5-flash",
            // gpt-oss-20b responds reliably on the free tier (verified 2026-10-08);
            // some larger NIM models (deepseek-v4.1-flash, kimi-k3) hang in queue.
            LlmProvider::Custom => "openai/gpt-oss-20b",
            // 7B coding model — best quality/size balance for an offline
            // 'self coding' brain (Apache-2.0). Smaller sizes remain listed.
            LlmProvider::Local => "qwen2.5-coder:7b",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            LlmProvider::OpenAI => "https://api.openai.com/v1",
            LlmProvider::Anthropic => "https://api.anthropic.com/v1",
            LlmProvider::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            LlmProvider::Custom => "https://integrate.api.nvidia.com/v1",
            // Ollama's OpenAI-compatible endpoint. llama.cpp/LM Studio on
            // another port can be entered in AI Settings.
            LlmProvider::Local => "http://localhost:11434/v1",
        }
    }

    pub fn all() -> &'static [LlmProvider] {
        &[
            LlmProvider::OpenAI,
            LlmProvider::Anthropic,
            LlmProvider::Gemini,
            LlmProvider::Custom,
            LlmProvider::Local,
        ]
    }
}

impl Default for LlmProvider {
    fn default() -> Self {
        LlmProvider::OpenAI
    }
}

impl std::fmt::Display for LlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LlmProviderConfig {
    pub provider: LlmProvider,
    /// Serialized as `apiKey` (frontend + disk); `api_key` (legacy disk) still accepted.
    #[serde(alias = "api_key")]
    pub api_key: Option<String>,
    pub model: String,
    pub enabled: bool,
    /// Base URL for the Custom provider (OpenAI-compatible), e.g.
    /// `https://integrate.api.nvidia.com/v1` or `http://localhost:1234/v1`.
    /// Serialized as `baseUrl`; `base_url` (legacy disk) still accepted.
    #[serde(alias = "base_url")]
    pub base_url: Option<String>,
}

impl Default for LlmProviderConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::default(),
            api_key: None,
            model: LlmProvider::default().default_model().to_string(),
            enabled: false,
            base_url: None,
        }
    }
}

fn default_auto_switch_on_quota_hit() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LlmConfig {
    /// Serialized as `activeProvider`; `active_provider` (legacy disk) still accepted.
    #[serde(alias = "active_provider")]
    pub active_provider: LlmProvider,
    pub providers: HashMap<LlmProvider, LlmProviderConfig>,
    /// Auto-switch to the next best provider when the active model's limit
    /// trips (429 / quota) or it hangs. Defaults ON — also for older config
    /// files where the field is missing (`serde(default = ...)`).
    #[serde(default = "default_auto_switch_on_quota_hit")]
    pub auto_switch_on_quota_hit: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            active_provider: LlmProvider::default(),
            providers: HashMap::new(),
            auto_switch_on_quota_hit: true,
        }
    }
}

impl LlmConfig {
    pub fn get_active(&self) -> Option<&LlmProviderConfig> {
        self.providers.get(&self.active_provider)
    }

    pub fn get_active_base_url(&self) -> String {
        self.get_active()
            .and_then(|c| c.base_url.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.active_provider.default_base_url().to_string())
    }
}

// ---------------------------------------------------------------------------
// Chat types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub provider: String,
    pub tokens_used: Option<u32>,
    pub finish_reason: Option<String>,
    /// True when auto-switch answered on a non-active provider (quota hit).
    #[serde(default)]
    pub is_fallback: bool,
    /// Provider that was originally requested (the active one) when a
    /// fallback answered; `None` when no switch happened.
    #[serde(default)]
    pub fallback_from: Option<String>,
}

// ---------------------------------------------------------------------------
// Unified chat function
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatError {
    pub message: String,
    pub provider: String,
    pub error_type: String,
}

// ---------------------------------------------------------------------------
// Unified chat function
// ---------------------------------------------------------------------------

pub async fn chat(
    config: &LlmConfig,
    request: ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let provider_config = config
        .get_active()
        .ok_or_else(|| ChatError {
            message: "No active LLM provider configured".into(),
            provider: "none".into(),
            error_type: "configuration".into(),
        })?;

    if !provider_config.enabled {
        return Err(ChatError {
            message: "Active LLM provider is disabled".into(),
            provider: provider_config.provider.to_string(),
            error_type: "configuration".into(),
        });
    }

    // Local provider runs without any key and without the internet.
    let api_key = if matches!(provider_config.provider, LlmProvider::Local) {
        ""
    } else {
        provider_config
            .api_key
            .as_ref()
            .filter(|k| !k.is_empty())
            .map(String::as_str)
            .ok_or_else(|| ChatError {
                message: "No API key configured for active provider".into(),
                provider: provider_config.provider.to_string(),
                error_type: "configuration".into(),
            })?
    };

    let model = provider_config.model.clone();

    match &provider_config.provider {
        LlmProvider::OpenAI => call_openai(api_key, &model, &request).await,
        LlmProvider::Anthropic => call_anthropic(api_key, &model, &request).await,
        LlmProvider::Gemini => call_gemini(api_key, &model, &request).await,
        LlmProvider::Custom => {
            call_custom(config.get_active_base_url(), api_key, &model, &request).await
        }
        // Local runtime never needs a key: the empty string is by design.
        LlmProvider::Local => {
            // The saved base URL can go stale (provider defaults to Ollama's
            // 11434 while the bundled engine serves on 8080, or the runtime
            // restarted on another port). Resolve a live endpoint before the
            // call: saved endpoint first, then the known loopback sweep.
            let saved = config.get_active_base_url();
            let endpoint = crate::llama_local::resolve_local_endpoint(Some(&saved))
                .await
                .ok_or_else(|| ChatError {
                    message: "Local AI runtime not reachable — open AI Settings > Offline local model and click 🚀 Set up offline AI.".into(),
                    provider: "Local".into(),
                    error_type: "network".into(),
                })?;
            call_local(endpoint, &model, &request).await
        }
    }
}

/// Canonicalize any incoming config JSON into a clean [`LlmConfig`].
/// (Supersedes the former `normalize_legacy_provider` helper — legacy
/// openrouter/camelCase shaping happens here in one pass.)
///
/// Tolerates every shape seen in the wild:
/// - camelCase (`activeProvider`/`apiKey`/`baseUrl`) from the frontend, snake
///   (`active_provider`/`api_key`/`base_url`) from old disk files — **or both
///   at once**, which plain serde would reject as a duplicate field.
/// - provider entries missing `provider`/`model` (older UI bug dropped them).
/// - a provider tag that contradicts its map key (e.g. `openai` inside the
///   `gemini` slot) — the map key wins and the model is reset to that
///   provider's default.
/// - legacy `openrouter` map keys/tags, remapped to `custom`.
fn canonical_config_value(raw: serde_json::Value) -> Result<LlmConfig, String> {
    let Some(obj) = raw.as_object() else {
        return Ok(LlmConfig::default());
    };

    let mut clean = serde_json::Map::new();
    let active = obj
        .get("activeProvider")
        .or_else(|| obj.get("active_provider"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // Legacy "openrouter" active tags become "custom".
    let active = match active.as_deref() {
        Some("openrouter") => Some("custom".to_string()),
        other => other.map(str::to_string),
    };
    if let Some(active) = active {
        clean.insert("activeProvider".to_string(), serde_json::Value::String(active));
    }

    if let Some(providers) = obj.get("providers").and_then(|p| p.as_object()) {
        let mut clean_providers = serde_json::Map::new();
        for (key, entry) in providers {
            let pid = match key.as_str() {
                "openrouter" => "custom".to_string(),
                other => other.to_string(),
            };
            let provider_enum: LlmProvider =
                serde_json::from_value(serde_json::Value::String(pid.clone()))
                    .unwrap_or(LlmProvider::Custom);
            let Some(entry_obj) = entry.as_object() else { continue };

            let first_non_empty = |names: &[&str]| -> Option<String> {
                names
                    .iter()
                    .find_map(|name| {
                        entry_obj
                            .get(*name)
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
                    .filter(|s| !s.trim().is_empty())
            };
            let api_key = first_non_empty(&["apiKey", "api_key"]);
            let base_url = first_non_empty(&["baseUrl", "base_url"]);
            let model = first_non_empty(&["model"]);
            let enabled = entry_obj
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let provider_tag = entry_obj
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // "openrouter" is a legacy alias of "custom", never a mismatch.
            let legacy_openrouter = key == "openrouter" || provider_tag == "openrouter";
            // A tag that contradicts the map key means the entry was written
            // by the field-dropping bug — distrust its model as well.
            let tag_mismatch =
                !provider_tag.is_empty() && provider_tag != pid && !legacy_openrouter;

            let model = match model {
                Some(model) if !tag_mismatch => model,
                _ => provider_enum.default_model().to_string(),
            };

            let mut clean_entry = serde_json::Map::new();
            clean_entry.insert(
                "provider".to_string(),
                serde_json::Value::String(pid.clone()),
            );
            if let Some(api_key) = api_key {
                clean_entry.insert("apiKey".to_string(), serde_json::Value::String(api_key));
            }
            clean_entry.insert("model".to_string(), serde_json::Value::String(model));
            clean_entry.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
            let base_url = base_url.or_else(|| {
                if legacy_openrouter {
                    Some("https://openrouter.ai/api/v1".to_string())
                } else if matches!(
                    provider_enum,
                    LlmProvider::Custom | LlmProvider::Local
                ) {
                    Some(provider_enum.default_base_url().to_string())
                } else {
                    None
                }
            });
            if let Some(base_url) = base_url {
                clean_entry.insert("baseUrl".to_string(), serde_json::Value::String(base_url));
            }
            clean_providers.insert(pid, serde_json::Value::Object(clean_entry));
        }
        clean.insert("providers".to_string(), serde_json::Value::Object(clean_providers));
    }

    serde_json::from_value(serde_json::Value::Object(clean)).map_err(|e| e.to_string())
}

/// Loads LLM config from disk, canonicalizing legacy/mixed spellings.
pub fn load_config(path: &std::path::Path) -> LlmConfig {
    let data = std::fs::read_to_string(path).unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&data) {
        Ok(value) => canonical_config_value(value).unwrap_or_default(),
        Err(_) => LlmConfig::default(),
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider (used by OpenAI and Custom endpoints)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

#[derive(Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    model: String,
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    total_tokens: Option<u32>,
}

async fn call_openai(
    api_key: &str,
    model: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let messages: Vec<OpenAiMessage> = request
        .messages
        .iter()
        .map(|m| OpenAiMessage {
            role: match m.role {
                MessageRole::System => "system".into(),
                MessageRole::User => "user".into(),
                MessageRole::Assistant => "assistant".into(),
            },
            content: m.content.clone(),
        })
        .collect();

    let body = OpenAiRequest {
        model: model.to_string(),
        messages,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
    };

    let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ChatError {
                message: format!("failed to build HTTP client: {}", e),
                provider: "network".into(),
                error_type: "network".into(),
            })?;
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChatError {
            message: format!("Request failed: {}", e),
            provider: "OpenAI".into(),
            error_type: "network".into(),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ChatError {
            message: format!("API error ({}): {}", status, text),
            provider: "OpenAI".into(),
            error_type: "api".into(),
        });
    }

    let data: OpenAiResponse = resp.json().await.map_err(|e| ChatError {
        message: format!("Failed to parse response: {}", e),
        provider: "OpenAI".into(),
        error_type: "parse".into(),
    })?;

    let content = data
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    let finish_reason = data
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone());

    Ok(ChatResponse {
        content,
        model: data.model,
        provider: "OpenAI".into(),
        tokens_used: data.usage.and_then(|u| u.total_tokens),
        finish_reason,
        is_fallback: false,
        fallback_from: None,
    })
}

// ---------------------------------------------------------------------------
// Custom OpenAI-compatible provider (Nvidia NIM, LM Studio, Ollama, vLLM, ...)
// ---------------------------------------------------------------------------

async fn call_custom(
    base_url: String,
    api_key: &str,
    model: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let messages: Vec<OpenAiMessage> = request
        .messages
        .iter()
        .map(|m| OpenAiMessage {
            role: match m.role {
                MessageRole::System => "system".into(),
                MessageRole::User => "user".into(),
                MessageRole::Assistant => "assistant".into(),
            },
            content: m.content.clone(),
        })
        .collect();

    let body = OpenAiRequest {
        model: model.to_string(),
        messages,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
    };

    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");
    let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ChatError {
                message: format!("failed to build HTTP client: {}", e),
                provider: "network".into(),
                error_type: "network".into(),
            })?;
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json");
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| ChatError {
            message: format!("Request failed: {}", e),
            provider: "Custom".into(),
            error_type: "network".into(),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ChatError {
            message: format!("API error ({}): {}", status, text),
            provider: "Custom".into(),
            error_type: "api".into(),
        });
    }

    let data: OpenAiResponse = resp.json().await.map_err(|e| ChatError {
        message: format!("Failed to parse response: {}", e),
        provider: "Custom".into(),
        error_type: "parse".into(),
    })?;

    let content = data
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    let finish_reason = data
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone());

    Ok(ChatResponse {
        content,
        model: data.model,
        provider: "Custom".into(),
        tokens_used: data.usage.and_then(|u| u.total_tokens),
        finish_reason,
        is_fallback: false,
        fallback_from: None,
    })
}

// ---------------------------------------------------------------------------
// Offline local runtime (Ollama / llama.cpp llama-server / LM Studio).
// Sends the same OpenAI-compatible payload to a localhost endpoint — no API
// key, no DNS, no internet. Kept separate from `call_custom` so its error
// messages can point at runtime setup instead of cloud keys.
// ---------------------------------------------------------------------------

async fn call_local(
    base_url: String,
    model: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let messages: Vec<OpenAiMessage> = request
        .messages
        .iter()
        .map(|m| OpenAiMessage {
            role: match m.role {
                MessageRole::System => "system".into(),
                MessageRole::User => "user".into(),
                MessageRole::Assistant => "assistant".into(),
            },
            content: m.content.clone(),
        })
        .collect();

    let body = OpenAiRequest {
        model: model.to_string(),
        messages,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
    };

    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");
    // Local inference is slower than a cloud API (CPU fallback especially) —
    // long timeout, short connect timeout so a dead runtime fails fast and
    // the fallback chain can take over.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| ChatError {
            message: format!("failed to build HTTP client: {}", e),
            provider: "network".into(),
            error_type: "network".into(),
        })?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer local") // some clients demand one
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            let msg = e.to_string();
            ChatError {
                message: if msg.contains("timed out") || msg.contains("connect") {
                    "Local AI runtime not reachable — start Ollama (or llama-server / LM Studio) or open AI Settings > Offline local model to set it up.".into()
                } else {
                    format!("Local runtime request failed: {}", e)
                },
                provider: "Local".into(),
                error_type: "network".into(),
            }
        })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ChatError {
            message: format!("Local runtime error ({}): {}", status, text),
            provider: "Local".into(),
            error_type: "api".into(),
        });
    }

    let data: OpenAiResponse = resp.json().await.map_err(|e| ChatError {
        message: format!("Failed to parse local runtime response: {}", e),
        provider: "Local".into(),
        error_type: "parse".into(),
    })?;

    let content = data
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    let finish_reason = data
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone());

    Ok(ChatResponse {
        content,
        model: data.model,
        provider: "Local".into(),
        tokens_used: data.usage.and_then(|u| u.total_tokens),
        finish_reason,
        is_fallback: false,
        fallback_from: None,
    })
}

// ---------------------------------------------------------------------------
// Anthropic provider
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    temperature: Option<f32>,
}

#[derive(Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    model: String,
    usage: Option<AnthropicUsage>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    text: String,
    #[serde(rename = "type")]
    block_type: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

async fn call_anthropic(
    api_key: &str,
    model: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let mut system_prompt: Option<String> = None;
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    for msg in &request.messages {
        match msg.role {
            MessageRole::System => {
                system_prompt = Some(msg.content.clone());
            }
            MessageRole::User => {
                messages.push(AnthropicMessage {
                    role: "user".into(),
                    content: msg.content.clone(),
                });
            }
            MessageRole::Assistant => {
                messages.push(AnthropicMessage {
                    role: "assistant".into(),
                    content: msg.content.clone(),
                });
            }
        }
    }

    let body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: request.max_tokens.unwrap_or(4096),
        system: system_prompt,
        messages,
        temperature: request.temperature,
    };

    let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ChatError {
                message: format!("failed to build HTTP client: {}", e),
                provider: "network".into(),
                error_type: "network".into(),
            })?;
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ChatError {
            message: format!("Request failed: {}", e),
            provider: "Anthropic".into(),
            error_type: "network".into(),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ChatError {
            message: format!("API error ({}): {}", status, text),
            provider: "Anthropic".into(),
            error_type: "api".into(),
        });
    }

    let data: AnthropicResponse = resp.json().await.map_err(|e| ChatError {
        message: format!("Failed to parse response: {}", e),
        provider: "Anthropic".into(),
        error_type: "parse".into(),
    })?;

    let content = data
        .content
        .iter()
        .filter(|b| b.block_type == "text")
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("");

    let tokens_used = data
        .usage
        .as_ref()
        .and_then(|u| u.input_tokens.zip(u.output_tokens))
        .map(|(i, o)| i + o);

    Ok(ChatResponse {
        content,
        model: data.model,
        provider: "Anthropic".into(),
        tokens_used,
        finish_reason: data.stop_reason,
        is_fallback: false,
        fallback_from: None,
    })
}

// ---------------------------------------------------------------------------
// Google Gemini provider
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
    role: String,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    max_output_tokens: Option<u32>,
    temperature: Option<f32>,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiUsage {
    total_token_count: Option<u32>,
}

async fn call_gemini(
    api_key: &str,
    model: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let mut system_instruction: Option<GeminiContent> = None;
    let mut contents: Vec<GeminiContent> = Vec::new();

    for msg in &request.messages {
        match msg.role {
            MessageRole::System => {
                system_instruction = Some(GeminiContent {
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                    role: "user".into(),
                });
            }
            MessageRole::User => {
                contents.push(GeminiContent {
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                    role: "user".into(),
                });
            }
            MessageRole::Assistant => {
                contents.push(GeminiContent {
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                    role: "model".into(),
                });
            }
        }
    }

    let body = GeminiRequest {
        contents,
        system_instruction,
        generation_config: Some(GeminiGenerationConfig {
            max_output_tokens: request.max_tokens,
            temperature: request.temperature,
        }),
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ChatError {
                message: format!("failed to build HTTP client: {}", e),
                provider: "network".into(),
                error_type: "network".into(),
            })?;
    // Free-tier Gemini frequently returns 429 (quota) / 503 (high demand).
    // Retry those with a short backoff instead of failing the first time.
    let mut last_error: Option<ChatError> = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500 * u64::from(attempt))).await;
        }
        let resp = match client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                last_error = Some(ChatError {
                    message: format!("Request failed: {}", e),
                    provider: "Gemini".into(),
                    error_type: "network".into(),
                });
                continue;
            }
        };

        let status = resp.status();
        if status.as_u16() == 429 || status.as_u16() == 503 {
            let text = resp.text().await.unwrap_or_default();
            last_error = Some(ChatError {
                message: format!("API error ({}): {}", status, text),
                provider: "Gemini".into(),
                error_type: "rate_limit".into(),
            });
            continue;
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ChatError {
                message: format!("API error ({}): {}", status, text),
                provider: "Gemini".into(),
                error_type: "api".into(),
            });
        }

        let data: GeminiResponse = match resp.json().await {
            Ok(data) => data,
            Err(e) => {
                return Err(ChatError {
                    message: format!("Failed to parse response: {}", e),
                    provider: "Gemini".into(),
                    error_type: "parse".into(),
                });
            }
        };

        let content = data
            .candidates
            .first()
            .and_then(|c| c.content.as_ref())
            .and_then(|c| c.parts.first())
            .map(|p| p.text.clone())
            .unwrap_or_default();

        let finish_reason = data.candidates.first().and_then(|c| c.finish_reason.clone());

        return Ok(ChatResponse {
            content,
            model: model.to_string(),
            provider: "Gemini".into(),
            tokens_used: data
                .usage_metadata
                .and_then(|u| u.total_token_count),
            finish_reason,
            is_fallback: false,
            fallback_from: None,
        });
    }

    Err(last_error.unwrap_or(ChatError {
        message: "Gemini request failed after retries".into(),
        provider: "Gemini".into(),
        error_type: "network".into(),
    }))
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Tauri command wrappers matching frontend expectations
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_llm_config(state: tauri::State<'_, crate::AppState>) -> LlmConfig {
    state.llm_config.lock().unwrap().clone()
}

/// Persist config JSON, creating parent directories when missing.
fn write_config_file(path: &std::path::Path, config: &LlmConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    std::fs::write(path, json).map_err(|e| format!("failed to write config: {e}"))
}

#[tauri::command]
pub fn save_llm_config(
    state: tauri::State<'_, crate::AppState>,
    config: serde_json::Value,
) -> Result<LlmConfig, String> {
    // Accept any payload shape (camelCase, snake_case, or both at once) and
    // canonicalize before persisting — never fail on duplicate spellings.
    let config: LlmConfig = canonical_config_value(config)?;
    write_config_file(&state.llm_config_path, &config)?;
    *state.llm_config.lock().unwrap() = config.clone();
    Ok(config)
}

/// Chat with the auto-switch fallback chain (direct replies).
///
/// Tries the active provider first; on transient failures (429 / quota,
/// network, hang-timeout) it walks `ranked_fallback_configs` — free-tier
/// providers before paid ones, each with its best free model. The response
/// carries `is_fallback` + `fallback_from` so the UI can badge the reply.
/// When auto-switching is disabled this behaves exactly like `chat`.
pub async fn chat_with_fallback(
    config: &LlmConfig,
    request: ChatRequest,
) -> Result<ChatResponse, ChatError> {
    let active_id = config.active_provider.clone();
    let chain = ranked_fallback_configs(config);

    // Chain[0] is the active provider (ranked list always leads with it).
    // With auto-switch off, ranked_fallback_configs returns empty and this is
    // just the plain single-provider call.
    let mut requested_active = true;
    for provider_cfg in &chain {
        let trial_config = LlmConfig {
            active_provider: provider_cfg.provider.clone(),
            providers: {
                let mut m = HashMap::new();
                m.insert(provider_cfg.provider.clone(), provider_cfg.clone());
                m
            },
            // Irrelevant here — the trial already picked its provider.
            auto_switch_on_quota_hit: false,
        };
        match chat(&trial_config, request.clone()).await {
            Ok(mut resp) => {
                if !requested_active && provider_cfg.provider != active_id {
                    resp.is_fallback = true;
                    resp.fallback_from = Some(active_id.to_string());
                }
                return Ok(resp);
            }
            Err(err) => {
                tracing::warn!(
                    "chat_with_fallback provider failed: provider={provider} error_type={error_type}",
                    provider = provider_cfg.provider.to_string(),
                    error_type = err.error_type,
                );
                requested_active = false;
                // Fall through to the next provider in the chain.
            }
        }
    }

    // Empty chain (auto-switch off, or nothing configured): single try.
    chat(config, request).await
}

#[tauri::command]
pub async fn llm_chat_command(
    messages: Vec<ChatMessage>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
) -> Result<ChatResponse, ChatError> {
    // Load config from the default path, migrating legacy openrouter entries.
    let config_path = crate::config_dir().join("llm_config.json");
    let config: LlmConfig = if config_path.exists() {
        load_config(&config_path)
    } else {
        LlmConfig::default()
    };
    let request = ChatRequest { messages, max_tokens, temperature };
    chat_with_fallback(&config, request).await
}


/// Central, single source of truth for LLM provider model lists.
/// Kept here (not duplicated in frontend) so the app can refresh it at runtime
/// and so every dropdown/card reads from one place.
///
/// Add or reorder models here and every UI that calls `llm_list_providers`
/// picks them up automatically — no frontend hardcoded list to keep in sync.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderInfo {
    pub id: String,
    pub name: String,
    pub default_model: String,
    pub api_base_url: String,
    pub models: Vec<String>,
    pub icon: String,
    /// Per-model metadata for auto-selection (cost, speed, capability).
    pub model_metadata: Vec<ModelInfo>,
}

impl LlmProviderInfo {
    pub fn from_provider(provider: &LlmProvider, models: Vec<String>) -> Self {
        let icon = match provider {
            LlmProvider::OpenAI => "🤖",
            LlmProvider::Anthropic => "🧠",
            LlmProvider::Gemini => "✨",
            LlmProvider::Custom => "🛠️",
            LlmProvider::Local => "🏠",
        };
        let metadata = models.iter().map(|m| model_info_for(provider, m)).collect();
        Self {
            id: format!("{:?}", provider).to_lowercase(),
            name: provider.display_name().to_string(),
            default_model: provider.default_model().to_string(),
            api_base_url: provider.default_base_url().to_string(),
            models,
            icon: icon.to_string(),
            model_metadata: metadata,
        }
    }
}

/// Per-model metadata used by the auto-select fallback.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub name: String,
    /// Approx cost per 1M output tokens in USD. `None` = unknown/free-ish.
    pub cost_per_1m_tokens: Option<f64>,
    /// Relative speed class: lower = faster.
    pub speed_class: u8,
    /// Relative capability tier: higher = smarter.
    pub capability_tier: u8,
    /// Whether the model is usually available on a free / no-credit tier.
    pub free_tier_available: bool,
}

/// Metadata for every curated model. Costs are approximations for ranking,
/// not billing data; unknown cost = `None` (treated as free in ranking).
pub fn model_info_for(provider: &LlmProvider, model: &str) -> ModelInfo {
    let (cost, speed, tier, free) = match provider {
        LlmProvider::OpenAI => match model {
            "gpt-6-astra" => (Some(60.0), 3, 10, false),
            "gpt-6.1-sol" | "gpt-6-luna" => (Some(12.0), 2, 9, false),
            "gpt-5.6-sol" => (Some(8.0), 2, 8, false),
            "gpt-5.5" => (Some(5.0), 2, 8, false),
            "gpt-5.4" => (Some(2.5), 2, 7, false),
            "gpt-5.4-mini" | "gpt-5-mini" => (Some(0.4), 1, 6, false),
            "gpt-5" => (Some(2.0), 2, 7, false),
            "gpt-4.1" => (Some(2.0), 2, 6, false),
            "gpt-4o-mini" => (Some(0.6), 1, 5, false),
            _ => (None, 2, 6, false),
        },
        LlmProvider::Anthropic => match model {
            "claude-fable-5-1" => (Some(25.0), 3, 10, false),
            "claude-opus-5-5" => (Some(25.0), 3, 10, false),
            "claude-sonnet-5-5" => (Some(9.0), 2, 9, false),
            "claude-haiku-5-5" => (Some(1.0), 1, 7, false),
            "claude-opus-4-7" => (Some(15.0), 3, 8, false),
            "claude-sonnet-4-6" => (Some(3.0), 2, 7, false),
            _ => (None, 2, 7, false),
        },
        LlmProvider::Gemini => match model {
            "gemini-3.8-flash" => (None, 1, 9, true),
            "gemini-3.6-flash" | "gemini-3.5-flash" => (None, 1, 8, true),
            "gemini-3.5-flash-lite" | "gemini-3.1-flash-lite" => (None, 1, 6, true),
            "gemini-flash-lite-latest" | "gemini-flash-latest" => (None, 1, 7, true),
            "gemini-3-flash-preview" => (None, 2, 7, true),
            "gemini-3.1-pro-preview" => (None, 3, 9, true),
            _ => (None, 2, 7, true),
        },
        LlmProvider::Custom => match model {
            // NVIDIA NIM free tier — everything here is free.
            "openai/gpt-oss-20b" => (None, 1, 7, true),
            "z-ai/glm-5.3" => (None, 1, 8, true),
            "meta/llama-3.2-90b-vision-instruct" => (None, 2, 7, true),
            "mistralai/mistral-large-2-instruct" => (None, 2, 8, true),
            "nvidia/llama-3.1-nemotron-ultra-253b-v1" => (None, 3, 8, true),
            "nvidia/nemotron-3-super-120b-a12b" => (None, 2, 8, true),
            "deepseek-ai/deepseek-v3.2" => (None, 2, 8, true),
            "moonshotai/kimi-2.5" => (None, 2, 8, true),
            _ => (None, 2, 6, true),
        },
        LlmProvider::Local => match model {
            // All offline: cost 0, ranking prefers the biggest coder.
            "qwen2.5-coder:14b" => (None, 3, 8, true),
            "qwen2.5-coder:7b" => (None, 2, 7, true),
            "qwen2.5-coder:3b" => (None, 1, 5, true),
            "qwen2.5-coder:1.5b" => (None, 1, 4, true),
            "phi3.5:3.8b" => (None, 1, 5, true),
            "deepseek-coder-v2:16b-lite" => (None, 3, 6, true),
            _ => (None, 2, 5, true),
        },
    };
    ModelInfo {
        name: model.to_string(),
        cost_per_1m_tokens: cost,
        speed_class: speed,
        capability_tier: tier,
        free_tier_available: free,
    }
}

/// Ordered model lists, verified / curated per provider.
/// When a provider ships new models, update this one place (or refresh from a
/// manifest at runtime via `llm_refresh_model_lists`) and the app follows.
pub fn provider_models(provider: &LlmProvider) -> Vec<String> {
    match provider {
        // OpenAI family — verified 2026-10-08 against OpenAI API docs.
        // gpt-5.6-terra / gpt-5.6-luna were placeholder names; only gpt-5.6-sol
        // is an actual OpenAI release in the 5.6 slot.
        LlmProvider::OpenAI => vec![
            "gpt-6-astra".into(),
            "gpt-6.1-sol".into(),
            "gpt-6-luna".into(),
            "gpt-5.6-sol".into(),
            "gpt-5.5".into(),
            "gpt-5.4".into(),
            "gpt-5.4-mini".into(),
            "gpt-5".into(),
            "gpt-5-mini".into(),
            "gpt-4.1".into(),
            "gpt-4o-mini".into(),
        ],
        // Anthropic family.
        LlmProvider::Anthropic => vec![
            "claude-fable-5-1".into(),
            "claude-opus-5-5".into(),
            "claude-sonnet-5-5".into(),
            "claude-haiku-5-5".into(),
            "claude-opus-4-7".into(),
            "claude-sonnet-4-6".into(),
        ],
        // Gemini family — verified 2026-10-08 against Google AI docs.
        // gemini-3.7-flash was an early placeholder; the GA releases are 3.5,
        // 3.6 and 3.8 flash variants.
        LlmProvider::Gemini => vec![
            "gemini-3.8-flash".into(),
            "gemini-3.6-flash".into(),
            "gemini-3.5-flash".into(),
            "gemini-3.5-flash-lite".into(),
            "gemini-flash-lite-latest".into(),
            "gemini-flash-latest".into(),
            "gemini-3-flash-preview".into(),
            "gemini-3.1-pro-preview".into(),
            "gemini-3.1-flash-lite".into(),
        ],
        // OpenAI-compatible / Custom family — verified 2026-10-08 against
        // NVIDIA NIM (build.nvidia.com). 80+ free models available; these are
        // the most useful chat/llm ones from that catalog.
        //
        // NOTE: deepseek-ai/deepseek-v4.1-flash and moonshotai/kimi-k3 were
        // placeholder names. Real models in that slot are deepseek-v3.2 and
        // kimi-2.5 (both on NVIDIA NIM as of 2026-10-08).
        LlmProvider::Custom => vec![
            "openai/gpt-oss-20b".into(),
            "z-ai/glm-5.3".into(),
            "meta/llama-3.2-90b-vision-instruct".into(),
            "mistralai/mistral-large-2-instruct".into(),
            "nvidia/llama-3.1-nemotron-ultra-253b-v1".into(),
            "nvidia/nemotron-3-super-120b-a12b".into(),
            "deepseek-ai/deepseek-v3.2".into(),
            "moonshotai/kimi-2.5".into(),
        ],
        // Offline local family — names follow Ollama's model tags (which are
        // equally valid as llama.cpp model names). Everything here runs on the
        // user's own hardware with zero network access. Qwen2.5-Coder is
        // Apache-2.0 across 0.5b/1.5b/7b/14b (3b is research-licensed —
        // deliberately NOT listed); phi3.5 is MIT.
        LlmProvider::Local => vec![
            "qwen2.5-coder:7b".into(),
            "qwen2.5-coder:14b".into(),
            "qwen2.5-coder:3b".into(),
            "qwen2.5-coder:1.5b".into(),
            "phi3.5:3.8b".into(),
            "deepseek-coder-v2:16b-lite".into(),
        ],
    }
}

#[tauri::command]
pub fn llm_list_providers() -> Vec<LlmProviderInfo> {
    LlmProvider::all()
        .iter()
        .map(|p| LlmProviderInfo::from_provider(p, provider_models(p)))
        .collect()
}

/// Ranked fallback order for auto-selection when the active model's limit is
/// hit (429 / RESOURCE_EXHAUSTED) or it hangs.
///
/// Ranking rules (project-hisaab-se best-first):
/// 1. Providers that are actually configured (key/base URL) and enabled.
/// 2. Free-tier-capable providers before paid ones — kaam chalta rahe, bill na bane.
/// 3. Within a provider, the best model = highest capability tier among
///    free-tier models (falls back to `default_model` when nothing is free).
/// 4. The currently active provider is tried first regardless of ranking, so
///    normal behaviour is unchanged until its limit actually trips.
///
/// Returns an empty vec when auto-switching is disabled
/// (`auto_switch_on_quota_hit = false`) — callers then behave single-provider.
pub fn ranked_fallback_configs(config: &LlmConfig) -> Vec<LlmProviderConfig> {
    if !config.auto_switch_on_quota_hit {
        return Vec::new();
    }
    let mut candidates: Vec<(&LlmProviderConfig, u8, String)> = config
        .providers
        .values()
        .filter(|c| {
            c.enabled
                && (c.api_key.as_ref().is_some_and(|k| !k.is_empty())
                    || c.base_url.as_ref().is_some_and(|u| !u.trim().is_empty())
                    // Local needs neither a key nor a URL — being enabled is
                    // enough, that's the whole point of offline mode.
                    || matches!(c.provider, LlmProvider::Local))
        })
        .map(|c| {
            let models = provider_models(&c.provider);
            let infos: Vec<ModelInfo> = models.iter().map(|m| model_info_for(&c.provider, m)).collect();
            // Best free model = highest capability tier among free-tier models.
            let best_free = infos
                .iter()
                .filter(|i| i.free_tier_available)
                .max_by_key(|i| i.capability_tier)
                .cloned();
            // Rank: free tier first, then the provider's best capability.
            let tier = best_free.as_ref().map(|i| i.capability_tier).unwrap_or(0);
            let rank = u8::from(best_free.is_some()) * 100 + tier;
            let fallback_model = best_free
                .map(|i| i.name)
                .unwrap_or_else(|| c.provider.default_model().to_string());
            (c, rank, fallback_model)
        })
        .collect();

    // Highest rank first; stable so equal ranks keep insertion order.
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    let mut ordered: Vec<LlmProviderConfig> = candidates
        .into_iter()
        .map(|(c, _, model)| {
            let mut entry = (*c).clone();
            entry.model = model;
            entry
        })
        .collect();

    // Active provider always leads the chain — its own limit error is what
    // triggers this list in the first place, so it gets one clean first try.
    let active_id = config.active_provider.clone();
    if let Some(pos) = ordered.iter().position(|c| c.provider == active_id) {
        let entry = ordered.remove(pos);
        ordered.insert(0, entry);
    }
    ordered
}

/// Refresh the curated model lists for every provider (and the active one).
///
/// Right now this re-reads the same curated lists kept in `provider_models`,
/// but the shape is set up so a future manifest/remote fetch can slot in here
/// without changing the rest of the app. Call this after adding a new provider
/// key, or on demand from the AI Settings page, to make sure the UI shows the
/// latest known model slate.
#[tauri::command]
pub fn llm_refresh_model_lists() -> Vec<LlmProviderInfo> {
    llm_list_providers()
}

#[tauri::command]
pub async fn llm_test_connection(
    provider: LlmProvider,
    api_key: String,
    model: Option<String>,
) -> Result<ChatResponse, ChatError> {
    let config = LlmConfig {
        active_provider: provider.clone(),
        providers: {
            let mut m = HashMap::new();
            m.insert(
                provider.clone(),
                LlmProviderConfig {
                    provider: provider.clone(),
                    api_key: Some(api_key),
                    model: model.unwrap_or_else(|| provider.default_model().to_string()),
                    enabled: true,
                    base_url: None,
                },
            );
            m
        },
        // The Test button targets exactly one provider — never auto-switch.
        auto_switch_on_quota_hit: false,
    };

    let test_messages = vec![ChatMessage {
        role: MessageRole::User,
        content: "Say 'Connection successful' and nothing else.".into(),
    }];

    let request = ChatRequest {
        messages: test_messages,
        max_tokens: Some(50),
        temperature: Some(0.0),
    };

    chat(&config, request).await
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn legacy_openrouter_config_migrates_to_custom() {
        // Keys match what save_llm_config actually writes (snake_case).
        let legacy = r#"{
            "active_provider": "openrouter",
            "providers": {
                "openrouter": {
                    "provider": "openrouter",
                    "api_key": "sk-or-test",
                    "model": "openai/gpt-4o",
                    "enabled": true
                }
            }
        }"#;
        let dir = std::env::temp_dir().join("nexora_llm_migration_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("llm_config.json");
        std::fs::write(&path, legacy).unwrap();

        let config = load_config(&path);
        let _ = std::fs::remove_file(&path);

        // The whole config must NOT have reset to defaults.
        assert_eq!(config.active_provider, LlmProvider::Custom);
        let cfg = config.providers.get(&LlmProvider::Custom).expect("custom entry must exist after migration");
        assert_eq!(cfg.model, "openai/gpt-4o");
        assert!(cfg.enabled);
        assert_eq!(cfg.base_url.as_deref(), Some("https://openrouter.ai/api/v1"));
        assert_eq!(cfg.api_key.as_deref(), Some("sk-or-test"));
    }

    #[test]
    fn modern_config_loads_unchanged() {
        // Saved by the old frontend: camelCase apiKey inside the entry.
        let modern = r#"{
            "active_provider": "gemini",
            "providers": {
                "gemini": {
                    "provider": "gemini",
                    "apiKey": "g-key",
                    "model": "gemini-2.0-flash",
                    "enabled": true,
                    "base_url": null
                }
            }
        }"#;
        let dir = std::env::temp_dir().join("nexora_llm_migration_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("llm_config_modern.json");
        std::fs::write(&path, modern).unwrap();

        let config = load_config(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.active_provider, LlmProvider::Gemini);
        let cfg = config.providers.get(&LlmProvider::Gemini).expect("gemini entry");
        assert_eq!(cfg.api_key.as_deref(), Some("g-key"));
    }
}

// ---------------------------------------------------------------------------
// External URL opener (used by AI Settings "Get API key" links)
// ---------------------------------------------------------------------------

/// Validate that a URL is safe to hand to the OS browser handler.
///
/// Only plain `https://` URLs are accepted: this command is invoked from the
/// UI, so a crafted value could otherwise reach shell handlers (file://,
/// cmd:, javascript: and friends are rejected here).
fn validate_external_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if !trimmed.to_ascii_lowercase().starts_with("https://") {
        return Err(format!("only https:// URLs are allowed, got: {trimmed}"));
    }
    if trimmed
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '<' | '>'))
    {
        return Err("URL contains forbidden characters".into());
    }
    Ok(trimmed.to_string())
}

/// Open an https URL in the user's default browser ("Get API key" links).
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let safe = validate_external_url(&url)?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Start-Process -FilePath '{}'", safe),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("failed to open browser: {error}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(&safe)
            .spawn()
            .map_err(|error| format!("failed to open browser: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod external_url_tests {
    use super::validate_external_url;

    #[test]
    fn accepts_https_url() {
        assert!(validate_external_url("https://aistudio.google.com/apikey").is_ok());
    }

    #[test]
    fn rejects_non_https_schemes() {
        assert!(validate_external_url("http://example.com").is_err());
        assert!(validate_external_url("file:///C:/Windows/System32").is_err());
        assert!(validate_external_url("cmd:/start calc").is_err());
        assert!(validate_external_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn rejects_shell_metacharacters() {
        assert!(validate_external_url("https://example.com/?a=' quote").is_err());
        assert!(validate_external_url("https://example.com/?a=\"dquote").is_err());
        assert!(validate_external_url("https://example.com/?a=b c d").is_err());
        assert!(validate_external_url("https://example.com/<x>").is_err());
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            validate_external_url("  https://aistudio.google.com  ").unwrap(),
            "https://aistudio.google.com"
        );
    }
}

#[cfg(test)]
mod config_compat_tests {
    use super::{load_config, write_config_file, LlmConfig, LlmProvider};

    /// The frontend sends camelCase keys (activeProvider / apiKey / baseUrl).
    /// Saving must accept them instead of failing on missing `active_provider`.
    #[test]
    fn frontend_camelcase_config_deserializes() {
        let payload = r#"{
            "activeProvider": "gemini",
            "providers": {
                "gemini": {
                    "provider": "gemini",
                    "apiKey": "AIza-test",
                    "model": "gemini-2.0-flash",
                    "enabled": true
                },
                "custom": {
                    "provider": "custom",
                    "apiKey": "nvapi-test",
                    "model": "meta/llama-3.1-8b-instruct",
                    "enabled": false,
                    "baseUrl": "https://integrate.api.nvidia.com/v1"
                }
            }
        }"#;
        let config: LlmConfig = serde_json::from_str(payload).expect("camelCase payload must parse");
        assert!(matches!(config.active_provider, LlmProvider::Gemini));
        let gemini = config.providers.get(&LlmProvider::Gemini).expect("gemini entry");
        assert_eq!(gemini.api_key.as_deref(), Some("AIza-test"));
        assert!(gemini.enabled);
        let custom = config.providers.get(&LlmProvider::Custom).expect("custom entry");
        assert_eq!(custom.base_url.as_deref(), Some("https://integrate.api.nvidia.com/v1"));
    }

    /// Saving must create missing parent directories (first-run config dir).
    #[test]
    fn save_creates_missing_parent_dirs_and_roundtrips() {
        let root = std::env::temp_dir().join("nexora_llm_save_test");
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("nested").join("deeper").join("llm_config.json");
        write_config_file(&path, &LlmConfig::default()).expect("write must create dirs");
        assert!(path.exists());
        let loaded = load_config(&path);
        let _ = std::fs::remove_dir_all(&root);
        assert!(matches!(loaded.active_provider, LlmProvider::OpenAI));
    }
}

#[cfg(test)]
mod canonical_config_tests {
    use super::{canonical_config_value, LlmProvider};

    /// The exact failure from the field: both spellings of every key in one
    /// payload (mixed legacy data + camelCase struct default_href) used to
    /// fail with `duplicate field active_provider`.
    #[test]
    fn mixed_spellings_and_duplicate_keys_parse() {
        let payload = serde_json::json!({
            "activeProvider": "gemini",
            "active_provider": "gemini",
            "providers": {
                "gemini": {
                    "provider": "gemini",
                    "apiKey": "AIza-test",
                    "api_key": "AIza-legacy",
                    "model": "gemini-2.0-flash",
                    "enabled": true
                }
            }
        });
        let config = canonical_config_value(payload).expect("mixed payload must parse");
        assert!(matches!(config.active_provider, LlmProvider::Gemini));
        let gemini = config.providers.get(&LlmProvider::Gemini).expect("gemini");
        assert_eq!(gemini.api_key.as_deref(), Some("AIza-test"));
    }

    /// The corrupted slot from the field: gemini key/container holding
    /// provider=openai + gpt-4o. Map key wins, model resets to Gemini default.
    #[test]
    fn corrupt_provider_tag_is_repaired() {
        let payload = serde_json::json!({
            "activeProvider": "gemini",
            "providers": {
                "gemini": {
                    "provider": "openai",
                    "apiKey": "AQ.test-key-placeholder-DO-NOT-COMMIT-REAL-KEYS",
                    "model": "gpt-4o",
                    "enabled": true
                }
            }
        });
        let config = canonical_config_value(payload).expect("corrupt payload must parse");
        let gemini = config.providers.get(&LlmProvider::Gemini).expect("gemini");
        assert!(matches!(gemini.provider, LlmProvider::Gemini));
        assert_eq!(gemini_model_for_test(&gemini), "gemini-3.5-flash");
        assert!(gemini.enabled);
    }

    fn gemini_model_for_test(cfg: &super::LlmProviderConfig) -> &str {
        cfg.model.as_str()
    }
}

#[cfg(test)]
mod local_provider_tests {
    use super::*;

    /// A config saved with a local provider persists the exact tag, model
    /// and loopback base URL without any key requirement.
    #[test]
    fn local_provider_round_trips_through_canonical_config() {
        let payload = serde_json::json!({
            "activeProvider": "local",
            "providers": {
                "local": {
                    "provider": "local",
                    "model": "qwen2.5-coder:7b",
                    "baseUrl": "http://localhost:11434/v1",
                    "enabled": true
                }
            }
        });
        let config = canonical_config_value(payload).expect("local payload must parse");
        assert!(matches!(config.active_provider, LlmProvider::Local));
        let local = config.providers.get(&LlmProvider::Local).expect("local entry");
        assert!(matches!(local.provider, LlmProvider::Local));
        assert_eq!(local.model, "qwen2.5-coder:7b");
        assert_eq!(local.base_url.as_deref(), Some("http://localhost:11434/v1"));
    }

    /// The offline provider must qualify for the fallback chain with neither
    /// an API key nor a URL — being enabled is the entire configuration.
    #[test]
    fn local_provider_enters_fallback_chain_without_key_or_url() {
        let config = LlmConfig {
            active_provider: LlmProvider::Gemini,
            providers: [
                (LlmProvider::Gemini, LlmProviderConfig {
                    provider: LlmProvider::Gemini,
                    api_key: Some("k".into()),
                    model: "gemini-3.5-flash".into(),
                    enabled: true,
                    base_url: None,
                }),
                (LlmProvider::Local, LlmProviderConfig {
                    provider: LlmProvider::Local,
                    api_key: None,
                    model: "qwen2.5-coder:7b".into(),
                    enabled: true,
                    base_url: None,
                }),
            ]
            .into_iter()
            .collect(),
            auto_switch_on_quota_hit: true,
        };
        let chain = ranked_fallback_configs(&config);
        assert!(chain.iter().any(|c| matches!(c.provider, LlmProvider::Local)));
    }

    /// Disabled local stays out of the chain.
    #[test]
    fn disabled_local_skips_fallback_chain() {
        let config = LlmConfig {
            active_provider: LlmProvider::Gemini,
            providers: [
                (LlmProvider::Gemini, LlmProviderConfig {
                    provider: LlmProvider::Gemini,
                    api_key: Some("k".into()),
                    model: "gemini-3.5-flash".into(),
                    enabled: true,
                    base_url: None,
                }),
                (LlmProvider::Local, LlmProviderConfig {
                    provider: LlmProvider::Local,
                    api_key: None,
                    model: "qwen2.5-coder:7b".into(),
                    enabled: false,
                    base_url: None,
                }),
            ]
            .into_iter()
            .collect(),
            auto_switch_on_quota_hit: true,
        };
        let chain = ranked_fallback_configs(&config);
        assert!(!chain.iter().any(|c| matches!(c.provider, LlmProvider::Local)));
    }
}

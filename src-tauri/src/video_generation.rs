use reqwest::{StatusCode, Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    project::ProjectState,
    providers::LicenseMetadata,
    video_validation::{self, VideoMetadata},
};

pub const PROVIDER_ID: &str = "local.comfyui";
pub const PROVIDER_VERSION: &str = "1.0.0";
pub const VIDEO_JOB_TYPE: &str = "video.generate";
pub const PROFILE_ID: &str = "stock.wan2.1.t2v.1.3b.lowvram.v1";
pub const DIFFUSION_MODEL: &str = "wan2.1_t2v_1.3B_fp16.safetensors";
pub const TEXT_ENCODER_MODEL: &str = "umt5_xxl_fp8_e4m3fn_scaled.safetensors";
pub const VAE_MODEL: &str = "wan_2.1_vae.safetensors";
const MODEL_SHIFT: f64 = 8.0;
const STEPS: u32 = 4;
const CFG: f64 = 6.0;
const SAMPLER: &str = "uni_pc";
const SCHEDULER: &str = "simple";
const DENOISE: f64 = 1.0;
const BATCH_SIZE: u32 = 1;
const TEXT_ENCODER_DEVICE: &str = "cpu";
const OUTPUT_FORMAT: &str = "mp4";
const OUTPUT_CODEC: &str = "h264";
const CONFIG_SCHEMA_VERSION: u32 = 1;
const REQUEST_SCHEMA_VERSION: u32 = 1;
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 40 * 1024 * 1024;
const OUTPUT_NODE_ID: &str = "11";
const MAX_WORKLOAD: u64 = 832 * 480 * 25;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoProviderConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub provider_id: String,
    pub base_url: String,
    pub timeout_seconds: u64,
}

impl Default for VideoProviderConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            enabled: false,
            provider_id: PROVIDER_ID.into(),
            base_url: "http://127.0.0.1:8188".into(),
            timeout_seconds: 120,
        }
    }
}

impl VideoProviderConfig {
    pub fn validate(&self) -> Result<Url, VideoError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION || self.provider_id != PROVIDER_ID {
            return Err(VideoError::InvalidConfig(
                "unsupported schema or provider id".into(),
            ));
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            return Err(VideoError::InvalidConfig(
                "timeoutSeconds must be 1..300".into(),
            ));
        }
        let url = Url::parse(&self.base_url)
            .map_err(|_| VideoError::InvalidConfig("baseUrl is invalid".into()))?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]"))
            || url.port().is_none()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(VideoError::InvalidConfig("baseUrl must be numeric loopback HTTP with an explicit port and no path, credentials, query, or fragment".into()));
        }
        Ok(url)
    }
}

pub fn load_config(path: &Path) -> Result<VideoProviderConfig, VideoError> {
    if !path.exists() {
        let value = VideoProviderConfig::default();
        save_config(path, &value)?;
        return Ok(value);
    }
    let value: VideoProviderConfig = serde_json::from_slice(&fs::read(path)?)?;
    value.validate()?;
    Ok(value)
}

pub fn save_config(path: &Path, config: &VideoProviderConfig) -> Result<(), VideoError> {
    config.validate()?;
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| VideoError::InvalidConfig("config path has no parent".into()))?,
    )?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoMode {
    TextToVideo,
    ImageToVideo,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoGenerationRequest {
    pub schema_version: u32,
    pub mode: VideoMode,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub fps: u32,
    pub seed: Option<i64>,
    pub profile: String,
    pub source_asset_id: Option<String>,
}

impl VideoGenerationRequest {
    pub fn normalize(mut self) -> Result<Self, VideoError> {
        let source_shape = matches!(
            (&self.mode, &self.source_asset_id),
            (VideoMode::TextToVideo, None) | (VideoMode::ImageToVideo, Some(_))
        );
        if self.schema_version != REQUEST_SCHEMA_VERSION
            || self.profile != PROFILE_ID
            || !source_shape
            || !(1..=2000).contains(&self.prompt.chars().count())
            || self
                .negative_prompt
                .as_deref()
                .map(str::chars)
                .map(Iterator::count)
                .unwrap_or(0)
                > 2000
            || !(256..=832).contains(&self.width)
            || !(128..=480).contains(&self.height)
            || !self.width.is_multiple_of(16)
            || !self.height.is_multiple_of(16)
            || !(1..=81).contains(&self.frame_count)
            || !(self.frame_count - 1).is_multiple_of(4)
            || !(1..=24).contains(&self.fps)
            || u64::from(self.width) * u64::from(self.height) * u64::from(self.frame_count)
                > MAX_WORKLOAD
            || self.seed.is_some_and(|seed| seed < 0)
            || self.source_asset_id.as_deref().is_some_and(|id| {
                Uuid::parse_str(id)
                    .map(|uuid| uuid.to_string() != id)
                    .unwrap_or(true)
            })
        {
            return Err(VideoError::InvalidRequest(
                "request is outside video generation bounds".into(),
            ));
        }
        if self.seed.is_none() {
            let bytes = Uuid::now_v7().into_bytes();
            self.seed = Some(i64::from_be_bytes(bytes[..8].try_into().unwrap()) & i64::MAX);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoJobPayload {
    pub provider_id: String,
    pub provider_version: String,
    pub provider_license: LicenseMetadata,
    pub request: VideoGenerationRequest,
}

#[derive(Clone, Debug)]
pub struct SourceAsset {
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct GeneratedVideo {
    pub staging_path: PathBuf,
    pub bytes_len: u64,
    pub checksum: String,
    pub metadata: VideoMetadata,
    pub model_identifier: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProviderState {
    pub reachable: bool,
    pub compatible: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("invalid video provider config: {0}")]
    InvalidConfig(String),
    #[error("invalid video generation request: {0}")]
    InvalidRequest(String),
    #[error("local video provider unavailable: {0}")]
    Unavailable(String),
    #[error("local video provider is incompatible: {0}")]
    Incompatible(String),
    #[error("local video provider timed out")]
    Timeout,
    #[error("local video provider returned retryable status {0}")]
    Server(u16),
    #[error("local video provider rejected request with status {0}")]
    Rejected(u16),
    #[error("malformed local video provider output: {0}")]
    Malformed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http client error: {0}")]
    Http(String),
}

impl VideoError {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Timeout | Self::Server(_))
    }
}

fn client(config: &VideoProviderConfig) -> Result<Client, VideoError> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(config.timeout_seconds.min(10)))
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()
        .map_err(|e| VideoError::Http(e.to_string()))
}
fn endpoint(config: &VideoProviderConfig, path: &str) -> Result<Url, VideoError> {
    let mut url = config.validate()?;
    url.set_path(path);
    Ok(url)
}
fn status(value: StatusCode) -> Result<(), VideoError> {
    if value.is_success() {
        Ok(())
    } else if value.is_server_error()
        || value == StatusCode::REQUEST_TIMEOUT
        || value == StatusCode::TOO_MANY_REQUESTS
    {
        Err(VideoError::Server(value.as_u16()))
    } else {
        Err(VideoError::Rejected(value.as_u16()))
    }
}
fn transport(error: reqwest::Error) -> VideoError {
    if error.is_timeout() {
        VideoError::Timeout
    } else {
        VideoError::Unavailable(error.to_string())
    }
}

fn get_json(config: &VideoProviderConfig, path: &str) -> Result<serde_json::Value, VideoError> {
    let mut response = client(config)?
        .get(endpoint(config, path)?)
        .send()
        .map_err(transport)?;
    status(response.status())?;
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(VideoError::Malformed("JSON response exceeds 4 MiB".into()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| VideoError::Malformed("invalid JSON response".into()))
}

fn response_json(
    mut response: reqwest::blocking::Response,
) -> Result<serde_json::Value, VideoError> {
    status(response.status())?;
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(VideoError::Malformed("JSON response exceeds 4 MiB".into()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| VideoError::Malformed("invalid JSON response".into()))
}

pub fn health(config: &VideoProviderConfig) -> Result<(), VideoError> {
    if !config.enabled {
        return Err(VideoError::Unavailable("provider is disabled".into()));
    }
    let value = get_json(config, "/system_stats")?;
    if !value
        .get("system")
        .is_some_and(serde_json::Value::is_object)
        || !value
            .get("devices")
            .is_some_and(serde_json::Value::is_array)
    {
        return Err(VideoError::Unavailable(
            "system_stats does not identify a compatible ComfyUI service".into(),
        ));
    }
    Ok(())
}

pub fn compatibility(config: &VideoProviderConfig) -> Result<(), VideoError> {
    health(config)?;
    let value = get_json(config, "/object_info")?;
    check_node_schema(
        &value,
        "UNETLoader",
        &[("unet_name", "COMBO"), ("weight_dtype", "COMBO")],
        &["MODEL"],
        false,
    )?;
    require_combo(
        &value,
        "UNETLoader",
        "required",
        "unet_name",
        DIFFUSION_MODEL,
    )?;
    require_combo(&value, "UNETLoader", "required", "weight_dtype", "default")?;
    check_node_schema(
        &value,
        "CLIPLoader",
        &[("clip_name", "COMBO"), ("type", "COMBO")],
        &["CLIP"],
        false,
    )?;
    require_combo(
        &value,
        "CLIPLoader",
        "required",
        "clip_name",
        TEXT_ENCODER_MODEL,
    )?;
    require_combo(&value, "CLIPLoader", "required", "type", "wan")?;
    require_combo(&value, "CLIPLoader", "optional", "device", "default")?;
    require_combo(
        &value,
        "CLIPLoader",
        "optional",
        "device",
        TEXT_ENCODER_DEVICE,
    )?;
    check_node_schema(
        &value,
        "VAELoader",
        &[("vae_name", "COMBO")],
        &["VAE"],
        false,
    )?;
    require_combo(&value, "VAELoader", "required", "vae_name", VAE_MODEL)?;
    check_node_schema(
        &value,
        "CLIPTextEncode",
        &[("text", "STRING"), ("clip", "CLIP")],
        &["CONDITIONING"],
        false,
    )?;
    check_node_schema(
        &value,
        "ModelSamplingSD3",
        &[("model", "MODEL"), ("shift", "FLOAT")],
        &["MODEL"],
        false,
    )?;
    check_node_schema(
        &value,
        "EmptyHunyuanLatentVideo",
        &[
            ("width", "INT"),
            ("height", "INT"),
            ("length", "INT"),
            ("batch_size", "INT"),
        ],
        &["LATENT"],
        false,
    )?;
    check_node_schema(
        &value,
        "KSampler",
        &[
            ("model", "MODEL"),
            ("seed", "INT"),
            ("steps", "INT"),
            ("cfg", "FLOAT"),
            ("sampler_name", "COMBO"),
            ("scheduler", "COMBO"),
            ("positive", "CONDITIONING"),
            ("negative", "CONDITIONING"),
            ("latent_image", "LATENT"),
            ("denoise", "FLOAT"),
        ],
        &["LATENT"],
        false,
    )?;
    require_combo(&value, "KSampler", "required", "sampler_name", SAMPLER)?;
    require_combo(&value, "KSampler", "required", "scheduler", SCHEDULER)?;
    check_node_schema(
        &value,
        "VAEDecode",
        &[("samples", "LATENT"), ("vae", "VAE")],
        &["IMAGE"],
        false,
    )?;
    check_node_schema(
        &value,
        "CreateVideo",
        &[("images", "IMAGE"), ("fps", "FLOAT")],
        &["VIDEO"],
        false,
    )?;
    check_node_schema(
        &value,
        "SaveVideo",
        &[
            ("video", "VIDEO"),
            ("filename_prefix", "STRING"),
            ("format", "COMBO"),
            ("codec", "COMFY_DYNAMICCOMBO_V3"),
        ],
        &["VIDEO"],
        true,
    )?;
    require_combo(&value, "SaveVideo", "required", "format", OUTPUT_FORMAT)?;
    require_dynamic_combo(&value, "SaveVideo", "codec", OUTPUT_CODEC)?;
    Ok(())
}

fn check_node_schema(
    object_info: &serde_json::Value,
    class: &str,
    inputs: &[(&str, &str)],
    outputs: &[&str],
    output_node: bool,
) -> Result<(), VideoError> {
    let node = object_info
        .get(class)
        .and_then(|value| value.as_object())
        .ok_or_else(|| VideoError::Incompatible(format!("stock node {class} is absent")))?;
    if node
        .get("output_node")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        != output_node
    {
        return Err(VideoError::Incompatible(format!(
            "stock node {class} has an invalid output role"
        )));
    }
    let required = node
        .get("input")
        .and_then(|value| value.get("required"))
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            VideoError::Incompatible(format!("stock node {class} has no required input schema"))
        })?;
    for (name, kind) in inputs {
        let actual = required.get(*name).and_then(input_kind);
        if actual.as_deref() != Some(*kind) {
            return Err(VideoError::Incompatible(format!(
                "stock node {class} input {name} is not {kind}"
            )));
        }
    }
    let actual_outputs = node
        .get("output")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            VideoError::Incompatible(format!("stock node {class} has no output schema"))
        })?;
    if actual_outputs.len() != outputs.len()
        || actual_outputs
            .iter()
            .zip(outputs)
            .any(|(actual, expected)| actual.as_str() != Some(expected))
    {
        return Err(VideoError::Incompatible(format!(
            "stock node {class} has an invalid output schema"
        )));
    }
    Ok(())
}

fn input_kind(value: &serde_json::Value) -> Option<String> {
    let kind = value.as_array()?.first()?;
    if kind.is_array() {
        Some("COMBO".into())
    } else {
        kind.as_str().map(str::to_owned)
    }
}

fn require_combo(
    object_info: &serde_json::Value,
    class: &str,
    group: &str,
    input: &str,
    expected: &str,
) -> Result<(), VideoError> {
    let schema = object_info
        .get(class)
        .and_then(|v| v.get("input"))
        .and_then(|v| v.get(group))
        .and_then(|v| v.get(input))
        .and_then(|v| v.as_array());
    let offered = schema.is_some_and(|schema| {
        schema
            .first()
            .and_then(|value| value.as_array())
            .is_some_and(|options| options.iter().any(|value| value.as_str() == Some(expected)))
            || (schema.first().and_then(|value| value.as_str()) == Some("COMBO")
                && schema
                    .get(1)
                    .and_then(|value| value.get("options"))
                    .and_then(|value| value.as_array())
                    .is_some_and(|options| {
                        options.iter().any(|value| value.as_str() == Some(expected))
                    }))
    });
    if !offered {
        return Err(VideoError::Incompatible(format!(
            "stock node {class} input {input} does not offer {expected}"
        )));
    }
    Ok(())
}

fn require_dynamic_combo(
    object_info: &serde_json::Value,
    class: &str,
    input: &str,
    expected: &str,
) -> Result<(), VideoError> {
    let schema = object_info
        .get(class)
        .and_then(|value| value.get("input"))
        .and_then(|value| value.get("required"))
        .and_then(|value| value.get(input))
        .and_then(|value| value.as_array());
    let valid = schema.is_some_and(|schema| {
        schema.first().and_then(|value| value.as_str()) == Some("COMFY_DYNAMICCOMBO_V3")
            && schema
                .get(1)
                .and_then(|value| value.get("options"))
                .and_then(|value| value.as_array())
                .is_some_and(|options| {
                    options.iter().any(|value| {
                        value.get("key").and_then(|key| key.as_str()) == Some(expected)
                    })
                })
    });
    if !valid {
        return Err(VideoError::Incompatible(format!(
            "stock node {class} input {input} does not offer {expected}"
        )));
    }
    Ok(())
}

pub fn provider_state(config: &VideoProviderConfig) -> VideoProviderState {
    match health(config) {
        Err(error) => VideoProviderState {
            reachable: false,
            compatible: false,
            detail: Some(error.to_string()),
        },
        Ok(()) => match compatibility(config) {
            Ok(()) => VideoProviderState {
                reachable: true,
                compatible: true,
                detail: None,
            },
            Err(error) => VideoProviderState {
                reachable: true,
                compatible: false,
                detail: Some(error.to_string()),
            },
        },
    }
}

pub fn validate_source(
    project: &ProjectState,
    request: &VideoGenerationRequest,
) -> Result<Option<SourceAsset>, VideoError> {
    let Some(id) = request.source_asset_id.as_deref() else {
        return Ok(None);
    };
    let (path, stored_checksum): (String, String) = project.db.lock().unwrap().query_row(
        "SELECT managed_master_path,checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready' AND media_kind='image'",
        rusqlite::params![id, project.manifest.project_id], |row| Ok((row.get(0)?, row.get(1)?))
    ).map_err(|_| VideoError::InvalidRequest("source asset must be a ready managed image in this project".into()))?;
    let canonical = dunce::canonicalize(path)
        .map_err(|_| VideoError::InvalidRequest("source asset master is unavailable".into()))?;
    let masters = dunce::canonicalize(project.root.join("assets").join("masters"))?;
    if !canonical.starts_with(&masters) || !canonical.is_file() {
        return Err(VideoError::InvalidRequest(
            "source asset is outside managed project storage".into(),
        ));
    }
    if fs::metadata(&canonical)?.len() > MAX_SOURCE_BYTES {
        return Err(VideoError::InvalidRequest(
            "source image exceeds 40 MiB".into(),
        ));
    }
    let mut file = fs::File::open(&canonical)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(VideoError::InvalidRequest(
            "source image is empty or exceeds 40 MiB".into(),
        ));
    }
    let checksum = format!("{:x}", Sha256::digest(&bytes));
    if stored_checksum.len() != 64 || !stored_checksum.eq_ignore_ascii_case(&checksum) {
        return Err(VideoError::InvalidRequest(
            "source image checksum does not match the managed asset".into(),
        ));
    }
    image::load_from_memory(&bytes).map_err(|_| {
        VideoError::InvalidRequest("source asset is not a fully decodable image".into())
    })?;
    Ok(Some(SourceAsset { bytes }))
}

pub fn execute_http(
    job_id: &str,
    payload: &VideoJobPayload,
    source: Option<&SourceAsset>,
    config: &VideoProviderConfig,
) -> Result<Vec<u8>, VideoError> {
    validate_payload(payload)?;
    let client_id = Uuid::parse_str(job_id)
        .map_err(|_| VideoError::InvalidRequest("invalid job identity".into()))?;
    if client_id.to_string() != job_id {
        return Err(VideoError::InvalidRequest("invalid job identity".into()));
    }
    compatibility(config)?;
    if !matches!(payload.request.mode, VideoMode::TextToVideo) || source.is_some() {
        return Err(VideoError::InvalidRequest(
            "stock Wan profile supports text-to-video only".into(),
        ));
    }
    let request = &payload.request;
    let prompt = stock_graph(job_id, request);
    let http = client(config)?;
    let history = response_json(
        http.get(endpoint(config, "/history")?)
            .send()
            .map_err(transport)?,
    )?;
    let prompt = match existing_prompt(&history, job_id)? {
        Some(prompt) => prompt,
        None => {
            let submitted = response_json(
                http.post(endpoint(config, "/prompt")?)
                    .json(
                        &serde_json::json!({"prompt": prompt, "client_id": client_id.to_string()}),
                    )
                    .send()
                    .map_err(transport)?,
            )?;
            let prompt_id = submitted
                .get("prompt_id")
                .and_then(|value| value.as_str())
                .filter(|value| safe_prompt_id(value))
                .ok_or_else(|| {
                    VideoError::Malformed("prompt response has no safe prompt_id".into())
                })?
                .to_owned();
            ExistingPrompt::Pending(prompt_id)
        }
    };
    let descriptor = resolve_prompt(&http, config, prompt)?;
    download_video(&http, config, &descriptor)
}

enum ExistingPrompt {
    Complete(serde_json::Value),
    Pending(String),
}

fn existing_prompt(
    history: &serde_json::Value,
    job_id: &str,
) -> Result<Option<ExistingPrompt>, VideoError> {
    let entries = history
        .as_object()
        .ok_or_else(|| VideoError::Malformed("history response is not an object".into()))?;
    let mut complete = None;
    let mut pending = None;
    let mut rejected = false;
    for (key, entry) in entries {
        if entry
            .get("prompt")
            .and_then(|value| value.get(3))
            .and_then(|value| value.get("client_id"))
            .and_then(|value| value.as_str())
            != Some(job_id)
        {
            continue;
        }
        let prompt_id = entry
            .get("prompt")
            .and_then(|value| value.get(1))
            .and_then(|value| value.as_str())
            .filter(|value| *value == key && safe_prompt_id(value))
            .ok_or_else(|| VideoError::Malformed("history has an unsafe prompt_id".into()))?;
        let completed = entry
            .get("status")
            .and_then(|value| value.get("completed"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let state = entry
            .get("status")
            .and_then(|value| value.get("status_str"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if completed && state == "success" {
            complete.get_or_insert_with(|| stock_video_descriptor(entry));
        } else if completed || matches!(state, "error" | "failed") {
            rejected = true;
        } else {
            pending.get_or_insert_with(|| prompt_id.to_owned());
        }
    }
    if let Some(descriptor) = complete {
        return Ok(Some(ExistingPrompt::Complete(descriptor?)));
    }
    if rejected {
        return Err(VideoError::Rejected(422));
    }
    if let Some(prompt_id) = pending {
        return Ok(Some(ExistingPrompt::Pending(prompt_id)));
    }
    Ok(None)
}

fn resolve_prompt(
    http: &Client,
    config: &VideoProviderConfig,
    prompt: ExistingPrompt,
) -> Result<serde_json::Value, VideoError> {
    let prompt_id = match prompt {
        ExistingPrompt::Complete(descriptor) => return Ok(descriptor),
        ExistingPrompt::Pending(prompt_id) => prompt_id,
    };
    let started = Instant::now();
    loop {
        if started.elapsed() >= Duration::from_secs(config.timeout_seconds) {
            return Err(VideoError::Timeout);
        }
        let history = response_json(
            http.get(endpoint(config, &format!("/history/{prompt_id}"))?)
                .send()
                .map_err(transport)?,
        )?;
        if let Some(entry) = history.get(&prompt_id) {
            let completed = entry
                .get("status")
                .and_then(|value| value.get("completed"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let state = entry
                .get("status")
                .and_then(|value| value.get("status_str"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if completed && state == "success" {
                return stock_video_descriptor(entry);
            }
            if completed || matches!(state, "error" | "failed") {
                return Err(VideoError::Rejected(422));
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn download_video(
    http: &Client,
    config: &VideoProviderConfig,
    descriptor: &serde_json::Value,
) -> Result<Vec<u8>, VideoError> {
    let filename = descriptor
        .get("filename")
        .and_then(|value| value.as_str())
        .filter(|value| safe_filename(value))
        .ok_or_else(|| VideoError::Malformed("video output has an unsafe filename".into()))?;
    let subfolder = descriptor
        .get("subfolder")
        .and_then(|value| value.as_str())
        .ok_or_else(|| VideoError::Malformed("video output has no subfolder".into()))?;
    if !safe_subfolder(subfolder) {
        return Err(VideoError::Malformed(
            "video output has an unsafe subfolder".into(),
        ));
    }
    let output_type = descriptor
        .get("type")
        .and_then(|value| value.as_str())
        .filter(|value| matches!(*value, "output" | "temp"))
        .ok_or_else(|| VideoError::Malformed("video output has an invalid type".into()))?;
    let mut view = endpoint(config, "/view")?;
    view.query_pairs_mut()
        .append_pair("filename", filename)
        .append_pair("subfolder", subfolder)
        .append_pair("type", output_type);
    let mut response = http.get(view).send().map_err(transport)?;
    status(response.status())?;
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_VIDEO_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_VIDEO_BYTES {
        return Err(VideoError::Malformed(
            "video is empty or exceeds 256 MiB".into(),
        ));
    }
    Ok(bytes)
}

fn stock_graph(job_id: &str, request: &VideoGenerationRequest) -> serde_json::Value {
    serde_json::json!({
        "1": {"class_type":"UNETLoader","inputs":{"unet_name":DIFFUSION_MODEL,"weight_dtype":"default"}},
        "2": {"class_type":"ModelSamplingSD3","inputs":{"model":["1",0],"shift":MODEL_SHIFT}},
        "3": {"class_type":"CLIPLoader","inputs":{"clip_name":TEXT_ENCODER_MODEL,"type":"wan","device":TEXT_ENCODER_DEVICE}},
        "4": {"class_type":"VAELoader","inputs":{"vae_name":VAE_MODEL}},
        "5": {"class_type":"CLIPTextEncode","inputs":{"text":request.prompt,"clip":["3",0]}},
        "6": {"class_type":"CLIPTextEncode","inputs":{"text":request.negative_prompt.as_deref().unwrap_or(""),"clip":["3",0]}},
        "7": {"class_type":"EmptyHunyuanLatentVideo","inputs":{"width":request.width,"height":request.height,"length":request.frame_count,"batch_size":BATCH_SIZE}},
        "8": {"class_type":"KSampler","inputs":{"model":["2",0],"seed":request.seed.unwrap(),"steps":STEPS,"cfg":CFG,"sampler_name":SAMPLER,"scheduler":SCHEDULER,"positive":["5",0],"negative":["6",0],"latent_image":["7",0],"denoise":DENOISE}},
        "9": {"class_type":"VAEDecode","inputs":{"samples":["8",0],"vae":["4",0]}},
        "10": {"class_type":"CreateVideo","inputs":{"images":["9",0],"fps":request.fps as f64}},
        OUTPUT_NODE_ID: {"class_type":"SaveVideo","inputs":{"video":["10",0],"filename_prefix":format!("nexora-{job_id}"),"format":OUTPUT_FORMAT,"codec":OUTPUT_CODEC}}
    })
}

fn stock_video_descriptor(entry: &serde_json::Value) -> Result<serde_json::Value, VideoError> {
    let output = entry
        .get("outputs")
        .and_then(|v| v.get(OUTPUT_NODE_ID))
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            VideoError::Malformed("completed prompt has no fixed SaveVideo output".into())
        })?;
    let descriptors = output.get("images").and_then(|value| value.as_array());
    if !descriptors.is_some_and(|values| values.len() == 1 && values[0].is_object()) {
        return Err(VideoError::Malformed(
            "completed prompt has no unambiguous stock SaveVideo descriptor".into(),
        ));
    }
    let descriptor = &descriptors.unwrap()[0];
    let safe = descriptor
        .get("filename")
        .and_then(|value| value.as_str())
        .is_some_and(safe_filename)
        && descriptor
            .get("subfolder")
            .and_then(|value| value.as_str())
            .is_some_and(safe_subfolder)
        && descriptor
            .get("type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| matches!(value, "output" | "temp"));
    if !safe {
        return Err(VideoError::Malformed(
            "completed prompt has an unsafe stock SaveVideo descriptor".into(),
        ));
    }
    Ok(descriptor.clone())
}

fn safe_prompt_id(value: &str) -> bool {
    (Uuid::parse_str(value).is_ok() && value.len() <= 36)
        || (value.len() >= 8
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
}

fn safe_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value.bytes().any(|byte| matches!(byte, b'/' | b'\\' | 0))
}

fn safe_subfolder(value: &str) -> bool {
    value.is_empty()
        || (value.len() <= 512
            && !value.starts_with('/')
            && !value.starts_with('\\')
            && value.split(['/', '\\']).all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part.len() <= 255
                    && !part.bytes().any(|byte| byte == 0 || byte == b':')
            }))
}

pub fn validate_payload(payload: &VideoJobPayload) -> Result<(), VideoError> {
    if payload.provider_id != PROVIDER_ID
        || payload.provider_version != PROVIDER_VERSION
        || payload.request.seed.is_none()
    {
        return Err(VideoError::InvalidRequest(
            "invalid persisted provider metadata".into(),
        ));
    }
    payload.request.clone().normalize().map(|_| ())
}

pub fn generation_staging(project: &ProjectState, job_id: &str) -> Result<PathBuf, VideoError> {
    let id =
        Uuid::parse_str(job_id).map_err(|_| VideoError::InvalidRequest("invalid job id".into()))?;
    if id.to_string() != job_id {
        return Err(VideoError::InvalidRequest("invalid job id".into()));
    }
    let path = project
        .root
        .join(".nexora")
        .join("jobs")
        .join(job_id)
        .join("video-staging");
    fs::create_dir_all(&path)?;
    let path = dunce::canonicalize(path)?;
    if !path.starts_with(dunce::canonicalize(&project.root)?) {
        return Err(VideoError::InvalidRequest("unsafe staging path".into()));
    }
    Ok(path)
}

pub fn stage_and_validate(
    project: &ProjectState,
    job_id: &str,
    request: &VideoGenerationRequest,
    bytes: &[u8],
) -> Result<GeneratedVideo, VideoError> {
    let metadata =
        video_validation::validate_mp4(bytes).map_err(|e| VideoError::Malformed(e.to_string()))?;
    validate_requested_video(&metadata, request)?;
    let path = generation_staging(project, job_id)?.join("output.mp4");
    fs::write(&path, bytes)?;
    let canonical = dunce::canonicalize(path)?;
    if !canonical.starts_with(generation_staging(project, job_id)?) || !canonical.is_file() {
        return Err(VideoError::InvalidRequest(
            "unsafe staged video path".into(),
        ));
    }
    Ok(GeneratedVideo {
        staging_path: canonical,
        bytes_len: bytes.len() as u64,
        checksum: format!("{:x}", Sha256::digest(bytes)),
        metadata,
        model_identifier: Some(DIFFUSION_MODEL.into()),
    })
}

fn validate_requested_video(
    metadata: &VideoMetadata,
    request: &VideoGenerationRequest,
) -> Result<(), VideoError> {
    let fps_matches = u64::from(metadata.fps_numerator)
        == u64::from(request.fps) * u64::from(metadata.fps_denominator);
    let expected_duration_ms = (u64::from(request.frame_count) * 1000 + u64::from(request.fps) / 2)
        / u64::from(request.fps);
    let maximum_duration_ms = (u64::from(request.frame_count + 1) * 1000
        + u64::from(request.fps) / 2)
        / u64::from(request.fps);
    // Stock CreateVideo may retain one final frame interval in the MP4 track duration.
    if metadata.width != request.width
        || metadata.height != request.height
        || metadata.frame_count != u64::from(request.frame_count)
        || !fps_matches
        || metadata.duration_ms + 2 < expected_duration_ms
        || metadata.duration_ms > maximum_duration_ms + 2
    {
        return Err(VideoError::Malformed(format!(
            "video metadata does not match request: actual={}x{},{} frames,{}/{},{}ms requested={}x{},{} frames,{}/1,{}ms",
            metadata.width,
            metadata.height,
            metadata.frame_count,
            metadata.fps_numerator,
            metadata.fps_denominator,
            metadata.duration_ms,
            request.width,
            request.height,
            request.frame_count,
            request.fps,
            expected_duration_ms
        )));
    }
    Ok(())
}

pub fn purge_staging(project: &ProjectState, job_id: &str) {
    if let Ok(path) = generation_staging(project, job_id) {
        let _ = fs::remove_dir_all(path);
    }
}

pub fn promote(
    project: &ProjectState,
    job_id: &str,
    owner: &str,
    payload: &VideoJobPayload,
    output: &GeneratedVideo,
) -> Result<bool, VideoError> {
    if !project.execution_enabled() {
        return Ok(false);
    }
    let asset_id = Uuid::now_v7().to_string();
    let masters = project.root.join("assets").join("masters");
    fs::create_dir_all(&masters)?;
    let master = masters.join(format!("{asset_id}.mp4"));
    let temporary = masters.join(format!(".{asset_id}.mp4.tmp"));
    fs::copy(&output.staging_path, &temporary)?;
    fs::rename(&temporary, &master)?;
    let result = (|| -> Result<bool, VideoError> {
        let mut db = project.db.lock().unwrap();
        let tx = db
            .transaction()
            .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        let (state, cancelled): (String, bool) = tx
            .query_row(
                "SELECT status,cancellation_requested FROM jobs WHERE job_id=?1 AND owner_token=?2",
                rusqlite::params![job_id, owner],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        if state != "running" || cancelled {
            return Ok(false);
        }
        let now = now_ms();
        let settings = serde_json::to_string(&serde_json::json!({
            "request": &payload.request,
            "profile": {
                "id": PROFILE_ID,
                "comfyuiVersion": "0.33.0",
                "diffusionModel": DIFFUSION_MODEL,
                "textEncoderModel": TEXT_ENCODER_MODEL,
                "vaeModel": VAE_MODEL,
                "textEncoderDevice": TEXT_ENCODER_DEVICE,
                "shift": MODEL_SHIFT,
                "steps": STEPS,
                "cfg": CFG,
                "sampler": SAMPLER,
                "scheduler": SCHEDULER,
                "denoise": DENOISE,
                "batchSize": BATCH_SIZE,
                "outputFormat": OUTPUT_FORMAT,
                "outputCodec": OUTPUT_CODEC,
                "validatedFrameCount": output.metadata.frame_count
            }
        }))?;
        let meta = &output.metadata;
        tx.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind,media_container,media_format,media_width,media_height,duration_ms,fps_numerator,fps_denominator,validation_level,codec) VALUES (?1,?2,'generated-video.mp4',?3,?4,?5,?6,'ready','generated','video',?7,?8,?9,?10,?11,?12,?13,?14,?15)", rusqlite::params![asset_id, project.manifest.project_id, master.to_string_lossy(), output.bytes_len, output.checksum, now, meta.container, meta.format, meta.width, meta.height, meta.duration_ms, meta.fps_numerator, meta.fps_denominator, meta.validation_level, meta.codec]).map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        tx.execute("INSERT INTO asset_provenance (asset_id,parent_asset_id,source_type,provider_id,provider_version,model_identifier,provider_license_state,provider_license_ref,model_license_state,model_license_ref,commercial_use_allowed,prompt,negative_prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) VALUES (?1,?2,'generated',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,1,?14,?15,?16)", rusqlite::params![asset_id, payload.request.source_asset_id, payload.provider_id, payload.provider_version, output.model_identifier, format!("{:?}", payload.provider_license.status).to_ascii_lowercase(), payload.provider_license.source_reference, payload.provider_license.model_license.as_ref().map(|_| "known"), payload.provider_license.model_license, payload.provider_license.commercial_use_allowed, payload.request.prompt, payload.request.negative_prompt, payload.request.seed, settings, job_id, now]).map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        tx.execute("UPDATE jobs SET status='completed',updated_at_ms=?1,completed_at_ms=?1,progress=100,owner_token=NULL WHERE job_id=?2 AND status='running' AND owner_token=?3", rusqlite::params![now, job_id, owner]).map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        tx.execute("INSERT INTO job_events (job_id,event_type,from_status,to_status,attempt_count,created_at_ms) SELECT job_id,'completed','running','completed',attempt_count,?1 FROM jobs WHERE job_id=?2", rusqlite::params![now, job_id]).map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        tx.commit()
            .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&master);
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoGenerationResult {
    pub job_id: String,
    pub status: String,
    pub asset_ids: Vec<String>,
}

pub fn get_result(
    project: &ProjectState,
    job_id: &str,
) -> Result<VideoGenerationResult, VideoError> {
    Uuid::parse_str(job_id).map_err(|_| VideoError::InvalidRequest("invalid job id".into()))?;
    let db = project.db.lock().unwrap();
    let status = db
        .query_row(
            "SELECT status FROM jobs WHERE job_id=?1 AND job_type='video.generate'",
            [job_id],
            |row| row.get(0),
        )
        .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
    let mut statement = db
        .prepare(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1 ORDER BY asset_id",
        )
        .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
    let ids = statement
        .query_map([job_id], |row| row.get(0))
        .map_err(|e| VideoError::InvalidRequest(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| VideoError::InvalidRequest(e.to_string()))?;
    Ok(VideoGenerationResult {
        job_id: job_id.into(),
        status,
        asset_ids: ids,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{LicenseStatus, ProviderRegistry};
    use std::io::Cursor;
    use tempfile::tempdir;

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn request(mode: VideoMode, source_asset_id: Option<String>) -> VideoGenerationRequest {
        VideoGenerationRequest {
            schema_version: 1,
            mode,
            prompt: "test".into(),
            negative_prompt: None,
            width: 320,
            height: 192,
            frame_count: 9,
            fps: 8,
            seed: Some(1),
            profile: PROFILE_ID.into(),
            source_asset_id,
        }
    }

    fn payload(request: VideoGenerationRequest) -> VideoJobPayload {
        VideoJobPayload {
            provider_id: PROVIDER_ID.into(),
            provider_version: PROVIDER_VERSION.into(),
            provider_license: LicenseMetadata {
                status: LicenseStatus::Unknown,
                name: None,
                model_license: None,
                commercial_use_allowed: None,
                source_reference: None,
            },
            request,
        }
    }
    #[test]
    fn endpoint_and_request_are_strict() {
        let config = VideoProviderConfig::default();
        for value in [
            "https://127.0.0.1:8188",
            "http://localhost:8188",
            "http://127.0.0.1",
            "http://127.0.0.1:8188/x",
            "http://u@127.0.0.1:8188",
            "http://127.0.0.1:8188?q=1",
        ] {
            let mut bad = config.clone();
            bad.base_url = value.into();
            assert!(bad.validate().is_err(), "accepted {value}");
        }
        let request = VideoGenerationRequest {
            schema_version: 1,
            mode: VideoMode::TextToVideo,
            prompt: "test".into(),
            negative_prompt: None,
            width: 320,
            height: 192,
            frame_count: 9,
            fps: 8,
            seed: Some(1),
            profile: PROFILE_ID.into(),
            source_asset_id: None,
        };
        assert!(request.clone().normalize().is_ok());
        let mut bad_frames = request.clone();
        bad_frames.frame_count = 8;
        assert!(bad_frames.normalize().is_err());
        let mut excessive_workload = request.clone();
        excessive_workload.width = 832;
        excessive_workload.height = 480;
        excessive_workload.frame_count = 81;
        assert!(excessive_workload.normalize().is_err());
        let mut bad = request.clone();
        bad.mode = VideoMode::ImageToVideo;
        assert!(bad.normalize().is_err());
        let mut value = serde_json::to_value(request).unwrap();
        value["workflow"] = serde_json::json!({});
        assert!(serde_json::from_value::<VideoGenerationRequest>(value).is_err());
        assert!(safe_prompt_id("018f47d2-9b8c-7a11-8000-123456789abc"));
        assert!(!safe_prompt_id("../../history"));
        assert!(safe_filename("clip.mp4"));
        assert!(!safe_filename("../clip.mp4"));
        assert!(safe_subfolder("safe/nested"));
        assert!(!safe_subfolder("safe/../nested"));
        assert!(!safe_subfolder("safe//nested"));
    }
    #[test]
    fn disabled_provider_is_neither_reachable_nor_compatible() {
        let state = provider_state(&VideoProviderConfig::default());
        assert!(!state.reachable && !state.compatible);
    }

    #[test]
    fn reachable_comfyui_without_owned_nodes_is_incompatible() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let count = stream.read(&mut request).unwrap();
                let text = String::from_utf8_lossy(&request[..count]);
                let body = if text.starts_with("GET /system_stats ") {
                    r#"{"system":{},"devices":[]}"#
                } else {
                    "{}"
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let config = VideoProviderConfig {
            enabled: true,
            base_url: format!("http://{address}"),
            ..VideoProviderConfig::default()
        };
        let state = provider_state(&config);
        assert!(state.reachable);
        assert!(!state.compatible);
        assert!(state.detail.unwrap().contains("stock node"));
    }

    #[test]
    fn source_validation_is_read_only_managed_and_image_only() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Video").unwrap();
        let masters = project.root.join("assets/masters");
        fs::create_dir_all(&masters).unwrap();
        let id = Uuid::now_v7().to_string();
        let path = masters.join(format!("{id}.png"));
        let png = png_bytes();
        fs::write(&path, &png).unwrap();
        let checksum = format!("{:x}", Sha256::digest(&png));
        project.with_db(|db| db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'source.png',?3,?4,?5,1,'ready','image')", rusqlite::params![id, project.manifest.project_id, path.to_string_lossy(), png.len(), checksum]).map(|_| ())).unwrap();
        let before = fs::read(&path).unwrap();
        let source = validate_source(
            &project,
            &request(VideoMode::ImageToVideo, Some(id.clone())),
        )
        .unwrap()
        .unwrap();
        assert_eq!(source.bytes, before);
        assert_eq!(fs::read(&path).unwrap(), before);
        project
            .with_db(|db| {
                db.execute("UPDATE assets SET checksum='00' WHERE asset_id=?1", [&id])
                    .map(|_| ())
            })
            .unwrap();
        assert!(
            validate_source(
                &project,
                &request(VideoMode::ImageToVideo, Some(id.clone()))
            )
            .is_err()
        );
        fs::write(&path, b"corrupt").unwrap();
        let corrupt_checksum = format!("{:x}", Sha256::digest(b"corrupt"));
        project
            .with_db(|db| {
                db.execute(
                    "UPDATE assets SET checksum=?1 WHERE asset_id=?2",
                    rusqlite::params![corrupt_checksum, id],
                )
                .map(|_| ())
            })
            .unwrap();
        assert!(
            validate_source(
                &project,
                &request(VideoMode::ImageToVideo, Some(id.clone()))
            )
            .is_err()
        );
        project
            .with_db(|db| {
                db.execute(
                    "UPDATE assets SET media_kind='video' WHERE asset_id=?1",
                    [&id],
                )
                .map(|_| ())
            })
            .unwrap();
        assert!(validate_source(&project, &request(VideoMode::ImageToVideo, Some(id))).is_err());
        assert!(generation_staging(&project, "../escape").is_err());
    }

    #[test]
    fn comfyui_absent_history_submits_polls_and_downloads() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        const PROMPT_ID: &str = "018f47d2-9b8c-7a11-8000-123456789abc";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let video = crate::video_validation::structural_fixture();
        let expected_video = video.clone();
        let job_id = Uuid::now_v7().to_string();
        let expected_job_id = job_id.clone();
        let server = thread::spawn(move || {
            for request_number in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(header_end) =
                        request.windows(4).position(|value| value == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&request[..header_end + 4]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= header_end + 4 + length {
                            break;
                        }
                    }
                }
                let header_end = request
                    .windows(4)
                    .position(|value| value == b"\r\n\r\n")
                    .unwrap();
                let head = String::from_utf8_lossy(&request[..header_end]);
                let first = head.lines().next().unwrap();
                let body = &request[header_end + 4..];
                let response = match request_number {
                    0 => {
                        assert_eq!(first, "GET /system_stats HTTP/1.1");
                        br#"{"system":{},"devices":[]}"#.to_vec()
                    }
                    1 => {
                        assert_eq!(first, "GET /object_info HTTP/1.1");
                        serde_json::to_vec(&object_info_fixture()).unwrap()
                    }
                    2 => {
                        assert_eq!(first, "GET /history HTTP/1.1");
                        b"{}".to_vec()
                    }
                    3 => {
                        assert_eq!(first, "POST /prompt HTTP/1.1");
                        let value: serde_json::Value = serde_json::from_slice(body).unwrap();
                        assert_eq!(value["client_id"], expected_job_id);
                        assert_eq!(value["prompt"], expected_graph(&expected_job_id));
                        format!(r#"{{"prompt_id":"{PROMPT_ID}"}}"#).into_bytes()
                    }
                    4 => {
                        assert_eq!(first, format!("GET /history/{PROMPT_ID} HTTP/1.1"));
                        format!(r#"{{"{PROMPT_ID}":{{"status":{{"completed":true,"status_str":"success"}},"outputs":{{"11":{{"images":[{{"filename":"clip.mp4","subfolder":"nexora/jobs","type":"output"}}]}}}}}}}}"#).into_bytes()
                    }
                    _ => {
                        assert_eq!(
                            first,
                            "GET /view?filename=clip.mp4&subfolder=nexora%2Fjobs&type=output HTTP/1.1"
                        );
                        expected_video.clone()
                    }
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        let config = VideoProviderConfig {
            enabled: true,
            base_url: format!("http://{address}"),
            timeout_seconds: 5,
            ..VideoProviderConfig::default()
        };
        let result = execute_http(
            &job_id,
            &payload(request(VideoMode::TextToVideo, None)),
            None,
            &config,
        )
        .unwrap();
        assert_eq!(result, video);
        server.join().unwrap();
    }

    #[test]
    fn comfyui_completed_matching_history_skips_submit_and_downloads() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            thread,
        };
        const PROMPT_ID: &str = "6c6f528f-da84-4a47-a7d3-5c575b9dbc0a";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let video = crate::video_validation::structural_fixture();
        let expected_video = video.clone();
        let job_id = "01a00019-98e0-7a40-be92-d64cc2f752f4".to_owned();
        let expected_job_id = job_id.clone();
        let server = thread::spawn(move || {
            for request_number in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|value| value == b"\r\n\r\n") {
                        break;
                    }
                }
                let header_end = request
                    .windows(4)
                    .position(|value| value == b"\r\n\r\n")
                    .unwrap();
                let head = String::from_utf8_lossy(&request[..header_end]);
                let first = head.lines().next().unwrap();
                let response = match request_number {
                    0 => {
                        assert_eq!(first, "GET /system_stats HTTP/1.1");
                        br#"{"system":{},"devices":[]}"#.to_vec()
                    }
                    1 => {
                        assert_eq!(first, "GET /object_info HTTP/1.1");
                        serde_json::to_vec(&object_info_fixture()).unwrap()
                    }
                    2 => {
                        assert_eq!(first, "GET /history HTTP/1.1");
                        serde_json::to_vec(&serde_json::json!({
                            (PROMPT_ID): {
                                "prompt": [17, PROMPT_ID, {}, {"client_id":expected_job_id}, [OUTPUT_NODE_ID]],
                                "status":{"completed":true,"status_str":"success"},
                                "outputs":{"11":{"images":[{"filename":"clip.mp4","subfolder":"","type":"output"}]}}
                            }
                        }))
                        .unwrap()
                    }
                    _ => {
                        assert_eq!(
                            first,
                            "GET /view?filename=clip.mp4&subfolder=&type=output HTTP/1.1"
                        );
                        expected_video.clone()
                    }
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        let config = VideoProviderConfig {
            enabled: true,
            base_url: format!("http://{address}"),
            timeout_seconds: 5,
            ..VideoProviderConfig::default()
        };
        let result = execute_http(
            &job_id,
            &payload(request(VideoMode::TextToVideo, None)),
            None,
            &config,
        )
        .unwrap();
        assert_eq!(result, video);
        server.join().unwrap();
    }

    fn object_info_fixture() -> serde_json::Value {
        let types = |values: &[(&str, &str)]| {
            values
                .iter()
                .map(|(name, kind)| ((*name).to_string(), serde_json::json!([kind])))
                .collect::<serde_json::Map<_, _>>()
        };
        serde_json::json!({
            "UNETLoader": {"output_node":false,"output":["MODEL"],"input":{"required":{"unet_name":[[DIFFUSION_MODEL]],"weight_dtype":[["default","fp8_e4m3fn"]]}}},
            "CLIPLoader": {"output_node":false,"output":["CLIP"],"input":{"required":{"clip_name":[[TEXT_ENCODER_MODEL]],"type":[["stable_diffusion","wan"]]},"optional":{"device":[["default","cpu"]]}}},
            "VAELoader": {"output_node":false,"output":["VAE"],"input":{"required":{"vae_name":[[VAE_MODEL]]}}},
            "CLIPTextEncode": {"output_node":false,"output":["CONDITIONING"],"input":{"required":types(&[("text","STRING"),("clip","CLIP")])}},
            "ModelSamplingSD3": {"output_node":false,"output":["MODEL"],"input":{"required":types(&[("model","MODEL"),("shift","FLOAT")])}},
            "EmptyHunyuanLatentVideo": {"output_node":false,"output":["LATENT"],"input":{"required":types(&[("width","INT"),("height","INT"),("length","INT"),("batch_size","INT")])}},
            "KSampler": {"output_node":false,"output":["LATENT"],"input":{"required":{
                "model":["MODEL"],"seed":["INT"],"steps":["INT"],"cfg":["FLOAT"],
                "sampler_name":[["euler","uni_pc"]],"scheduler":[["normal","simple"]],
                "positive":["CONDITIONING"],"negative":["CONDITIONING"],"latent_image":["LATENT"],"denoise":["FLOAT"]}}},
            "VAEDecode": {"output_node":false,"output":["IMAGE"],"input":{"required":types(&[("samples","LATENT"),("vae","VAE")])}},
            "CreateVideo": {"output_node":false,"output":["VIDEO"],"input":{"required":types(&[("images","IMAGE"),("fps","FLOAT")])}},
            "SaveVideo": {"output_node":true,"output":["VIDEO"],"input":{"required":{
                "video":["VIDEO"],"filename_prefix":["STRING"],"format":["COMBO",{"options":["auto","mp4"]}],"codec":["COMFY_DYNAMICCOMBO_V3",{"options":[{"key":"auto","inputs":{"required":{}}},{"key":"h264","inputs":{"required":{}}}]}]}}}
        })
    }

    fn expected_graph(job_id: &str) -> serde_json::Value {
        serde_json::json!({
            "1":{"class_type":"UNETLoader","inputs":{"unet_name":DIFFUSION_MODEL,"weight_dtype":"default"}},
            "2":{"class_type":"ModelSamplingSD3","inputs":{"model":["1",0],"shift":8.0}},
            "3":{"class_type":"CLIPLoader","inputs":{"clip_name":TEXT_ENCODER_MODEL,"type":"wan","device":"cpu"}},
            "4":{"class_type":"VAELoader","inputs":{"vae_name":VAE_MODEL}},
            "5":{"class_type":"CLIPTextEncode","inputs":{"text":"test","clip":["3",0]}},
            "6":{"class_type":"CLIPTextEncode","inputs":{"text":"","clip":["3",0]}},
            "7":{"class_type":"EmptyHunyuanLatentVideo","inputs":{"width":320,"height":192,"length":9,"batch_size":1}},
            "8":{"class_type":"KSampler","inputs":{"model":["2",0],"seed":1,"steps":4,"cfg":6.0,"sampler_name":"uni_pc","scheduler":"simple","positive":["5",0],"negative":["6",0],"latent_image":["7",0],"denoise":1.0}},
            "9":{"class_type":"VAEDecode","inputs":{"samples":["8",0],"vae":["4",0]}},
            "10":{"class_type":"CreateVideo","inputs":{"images":["9",0],"fps":8.0}},
            "11":{"class_type":"SaveVideo","inputs":{"video":["10",0],"filename_prefix":format!("nexora-{job_id}"),"format":"mp4","codec":"h264"}}
        })
    }

    #[test]
    fn valid_structural_video_promotes_with_checksum_provenance_and_reopens() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        let project = ProjectState::create(&root, "Video").unwrap();
        let source_id = Uuid::now_v7().to_string();
        let source_path = project
            .root
            .join("assets/masters")
            .join(format!("{source_id}.png"));
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, b"source").unwrap();
        project.with_db(|db| db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'source.png',?3,6,'source',1,'ready','image')", rusqlite::params![source_id, project.manifest.project_id, source_path.to_string_lossy()]).map(|_| ())).unwrap();
        let job_id = Uuid::now_v7().to_string();
        project.with_db(|db| db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts,attempt_count,owner_token) VALUES (?1,'video.generate','running','{}',1,1,2,1,'worker')", [&job_id]).map(|_| ())).unwrap();
        let bytes = crate::video_validation::structural_fixture();
        let generation_request = request(VideoMode::ImageToVideo, Some(source_id.clone()));
        let output = stage_and_validate(&project, &job_id, &generation_request, &bytes).unwrap();
        let wrong_fps = VideoGenerationRequest {
            fps: 9,
            ..generation_request.clone()
        };
        assert!(stage_and_validate(&project, &job_id, &wrong_fps, &bytes).is_err());
        assert!(
            promote(
                &project,
                &job_id,
                "worker",
                &payload(generation_request),
                &output
            )
            .unwrap()
        );
        purge_staging(&project, &job_id);
        let result = get_result(&project, &job_id).unwrap();
        assert_eq!(result.asset_ids.len(), 1);
        let asset_id = result.asset_ids[0].clone();
        let stored: (String, String, String, i64, i64, i64) = project.with_db(|db| db.query_row("SELECT media_kind,validation_level,checksum,media_width,media_height,(SELECT COUNT(*) FROM asset_provenance WHERE asset_id=assets.asset_id AND generating_job_id=?2 AND parent_asset_id=?3) FROM assets WHERE asset_id=?1", rusqlite::params![asset_id, job_id, source_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))).unwrap();
        assert_eq!(
            (
                stored.0.as_str(),
                stored.1.as_str(),
                stored.3,
                stored.4,
                stored.5
            ),
            ("video", "structural", 320, 192, 1)
        );
        assert_eq!(stored.2, format!("{:x}", Sha256::digest(&bytes)));
        let (model, settings): (String, String) = project.with_db(|db| db.query_row(
            "SELECT model_identifier,generation_settings_json FROM asset_provenance WHERE asset_id=?1",
            [&asset_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(model, DIFFUSION_MODEL);
        assert_eq!(settings["request"]["frameCount"], 9);
        assert_eq!(settings["profile"]["id"], PROFILE_ID);
        assert_eq!(settings["profile"]["validatedFrameCount"], 9);
        assert!(
            !project
                .root
                .join("assets/previews")
                .join(format!("{asset_id}_preview.png"))
                .exists()
        );
        project.close().unwrap();
        let reopened = ProjectState::open(&root).unwrap();
        assert_eq!(
            get_result(&reopened, &job_id).unwrap().asset_ids,
            [asset_id]
        );
    }

    #[test]
    fn registry_does_not_claim_video_capability_from_enablement_alone() {
        let registry = ProviderRegistry::phase7(false, true).unwrap();
        assert!(
            registry
                .validate_video_generation(
                    PROVIDER_ID,
                    crate::providers::Capability::TextToVideo,
                    &crate::hardware::HardwareSnapshot::unknown()
                )
                .is_err()
        );

        let mut registry = ProviderRegistry::phase7(false, true).unwrap();
        registry
            .set_local_comfyui_state(true, true, true, None)
            .unwrap();
        assert!(
            registry
                .validate_video_generation(
                    PROVIDER_ID,
                    crate::providers::Capability::ImageToVideo,
                    &crate::hardware::HardwareSnapshot::unknown()
                )
                .is_err()
        );
    }
}

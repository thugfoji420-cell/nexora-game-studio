use base64::Engine;
use image::{GenericImageView, ImageFormat, ImageReader};
use reqwest::{StatusCode, Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{project::ProjectState, providers::LicenseMetadata};

pub const PROVIDER_ID: &str = "local.a1111";
pub const PROVIDER_VERSION: &str = "1.0.0";
pub const IMAGE_JOB_TYPE: &str = "image.generate";
const CONFIG_SCHEMA_VERSION: u32 = 1;
const REQUEST_SCHEMA_VERSION: u32 = 1;
const MAX_RESPONSE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageProviderConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub provider_id: String,
    pub base_url: String,
    pub timeout_seconds: u64,
}

impl Default for ImageProviderConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            enabled: false,
            provider_id: PROVIDER_ID.into(),
            base_url: "http://127.0.0.1:7860".into(),
            timeout_seconds: 60,
        }
    }
}

impl ImageProviderConfig {
    pub fn validate(&self) -> Result<Url, ImageError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION || self.provider_id != PROVIDER_ID {
            return Err(ImageError::InvalidConfig(
                "unsupported schema or provider id".into(),
            ));
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            return Err(ImageError::InvalidConfig(
                "timeoutSeconds must be 1..300".into(),
            ));
        }
        let url = Url::parse(&self.base_url)
            .map_err(|_| ImageError::InvalidConfig("baseUrl is invalid".into()))?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]"))
            || url.port().is_none()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(ImageError::InvalidConfig(
                "baseUrl must be loopback HTTP with an explicit port and no path, credentials, query, or fragment".into(),
            ));
        }
        Ok(url)
    }
}

pub fn load_config(path: &Path) -> Result<ImageProviderConfig, ImageError> {
    if !path.exists() {
        let config = ImageProviderConfig::default();
        save_config(path, &config)?;
        return Ok(config);
    }
    let config: ImageProviderConfig = serde_json::from_slice(&fs::read(path)?)?;
    config.validate()?;
    Ok(config)
}

pub fn save_config(path: &Path, config: &ImageProviderConfig) -> Result<(), ImageError> {
    config.validate()?;
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| ImageError::InvalidConfig("config path has no parent".into()))?,
    )?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(config)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageGenerationRequest {
    pub schema_version: u32,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub width: u32,
    pub height: u32,
    pub seed: Option<i64>,
    pub steps: u32,
    pub guidance: f32,
    pub output_count: u32,
}

impl ImageGenerationRequest {
    pub fn normalize(mut self) -> Result<Self, ImageError> {
        let prompt_chars = self.prompt.chars().count();
        let negative_chars = self
            .negative_prompt
            .as_deref()
            .map(str::chars)
            .map(Iterator::count)
            .unwrap_or(0);
        if self.schema_version != REQUEST_SCHEMA_VERSION
            || !(1..=2000).contains(&prompt_chars)
            || negative_chars > 2000
            || !(256..=1024).contains(&self.width)
            || !(256..=1024).contains(&self.height)
            || !self.width.is_multiple_of(64)
            || !self.height.is_multiple_of(64)
            || u64::from(self.width) * u64::from(self.height) * u64::from(self.output_count)
                > 2 * 1024 * 1024
            || !(1..=50).contains(&self.steps)
            || !self.guidance.is_finite()
            || !(1.0..=20.0).contains(&self.guidance)
            || !(1..=2).contains(&self.output_count)
            || self.seed.is_some_and(|seed| seed < 0)
        {
            return Err(ImageError::InvalidRequest(
                "request is outside image generation bounds".into(),
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
pub struct ImageJobPayload {
    pub provider_id: String,
    pub provider_version: String,
    pub provider_license: LicenseMetadata,
    pub request: ImageGenerationRequest,
}

#[derive(Clone, Debug)]
pub struct GeneratedOutput {
    pub staging_path: PathBuf,
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub checksum: String,
    pub actual_seed: i64,
}

#[derive(Clone, Debug)]
pub struct AdapterResult {
    pub outputs: Vec<GeneratedOutput>,
    pub model_identifier: Option<String>,
}

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("invalid image provider config: {0}")]
    InvalidConfig(String),
    #[error("invalid image generation request: {0}")]
    InvalidRequest(String),
    #[error("local image provider unavailable: {0}")]
    Unavailable(String),
    #[error("local image provider timed out")]
    Timeout,
    #[error("local image provider returned retryable status {0}")]
    Server(u16),
    #[error("local image provider rejected request with status {0}")]
    Rejected(u16),
    #[error("malformed local image provider output: {0}")]
    Malformed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http client error: {0}")]
    Http(String),
}

impl ImageError {
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Timeout | Self::Server(_))
    }
}

fn client(config: &ImageProviderConfig) -> Result<Client, ImageError> {
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(config.timeout_seconds.min(10)))
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()
        .map_err(|error| ImageError::Http(error.to_string()))
}

fn endpoint(config: &ImageProviderConfig, path: &str) -> Result<Url, ImageError> {
    let mut url = config.validate()?;
    url.set_path(path);
    Ok(url)
}

pub fn health(config: &ImageProviderConfig) -> Result<(), ImageError> {
    if !config.enabled {
        return Err(ImageError::Unavailable(
            "provider is not configured or enabled".into(),
        ));
    }
    let response = client(config)?
        .get(endpoint(config, "/sdapi/v1/options")?)
        .send()
        .map_err(classify_transport)?;
    classify_status(response.status())?;
    Ok(())
}

pub fn execute_http(
    payload: &ImageJobPayload,
    config: &ImageProviderConfig,
) -> Result<Vec<u8>, ImageError> {
    validate_payload(payload)?;
    if !config.enabled {
        return Err(ImageError::Unavailable("provider is disabled".into()));
    }
    let request = &payload.request;
    let body = serde_json::json!({
        "prompt": request.prompt,
        "negative_prompt": request.negative_prompt.as_deref().unwrap_or(""),
        "width": request.width,
        "height": request.height,
        "seed": request.seed.unwrap(),
        "steps": request.steps,
        "cfg_scale": request.guidance,
        "batch_size": request.output_count,
        "n_iter": 1,
        "send_images": true,
        "save_images": false
    });
    let mut response = client(config)?
        .post(endpoint(config, "/sdapi/v1/txt2img")?)
        .json(&body)
        .send()
        .map_err(classify_transport)?;
    classify_status(response.status())?;
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(classify_body_io)?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(ImageError::Malformed("response exceeds 40 MiB".into()));
    }
    Ok(bytes)
}

fn classify_body_io(error: std::io::Error) -> ImageError {
    match error.kind() {
        std::io::ErrorKind::TimedOut => ImageError::Timeout,
        std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::UnexpectedEof => ImageError::Unavailable(error.to_string()),
        _ => ImageError::Io(error),
    }
}

fn classify_transport(error: reqwest::Error) -> ImageError {
    if error.is_timeout() {
        ImageError::Timeout
    } else {
        ImageError::Unavailable(error.to_string())
    }
}

fn classify_status(status: StatusCode) -> Result<(), ImageError> {
    if status.is_success() {
        Ok(())
    } else if status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
    {
        Err(ImageError::Server(status.as_u16()))
    } else {
        Err(ImageError::Rejected(status.as_u16()))
    }
}

#[derive(Deserialize)]
struct A1111Response {
    images: Vec<String>,
    #[serde(default)]
    info: serde_json::Value,
}

pub(crate) fn parse_response(
    project: &ProjectState,
    job_id: &str,
    request: &ImageGenerationRequest,
    bytes: &[u8],
) -> Result<AdapterResult, ImageError> {
    let response: A1111Response = serde_json::from_slice(bytes)
        .map_err(|_| ImageError::Malformed("invalid response JSON".into()))?;
    if response.images.len() != request.output_count as usize {
        return Err(ImageError::Malformed("wrong output count".into()));
    }
    let info = response
        .info
        .as_str()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .or_else(|| response.info.is_object().then_some(response.info.clone()));
    let seeds = output_seeds(info.as_ref(), request)?;
    let staging = generation_staging(project, job_id)?;
    let mut outputs = Vec::with_capacity(response.images.len());
    for (index, encoded) in response.images.iter().enumerate() {
        let encoded = encoded
            .rsplit_once(',')
            .map_or(encoded.as_str(), |(_, value)| value);
        let decoded = base64::prelude::BASE64_STANDARD
            .decode(encoded)
            .map_err(|_| ImageError::Malformed("invalid base64 image".into()))?;
        if decoded.is_empty() || decoded.len() > MAX_OUTPUT_BYTES {
            return Err(ImageError::Malformed(
                "image is empty or exceeds 16 MiB".into(),
            ));
        }
        let format = image::guess_format(&decoded)
            .map_err(|_| ImageError::Malformed("unknown image format".into()))?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
            return Err(ImageError::Malformed(
                "only PNG and JPEG outputs are accepted".into(),
            ));
        }
        let reader = ImageReader::with_format(Cursor::new(&decoded), format);
        let (width, height) = reader
            .into_dimensions()
            .map_err(|_| ImageError::Malformed("corrupt image header".into()))?;
        if width != request.width
            || height != request.height
            || u64::from(width) * u64::from(height) > 1024 * 1024
        {
            return Err(ImageError::Malformed(
                "image dimensions do not match request".into(),
            ));
        }
        let image = image::load_from_memory_with_format(&decoded, format)
            .map_err(|_| ImageError::Malformed("corrupt image".into()))?;
        debug_assert_eq!(image.dimensions(), (width, height));
        let extension = if format == ImageFormat::Png {
            "png"
        } else {
            "jpg"
        };
        let path = staging.join(format!("output-{index}.{extension}"));
        fs::write(&path, &decoded)?;
        let canonical = dunce::canonicalize(&path)?;
        if !canonical.starts_with(&staging) {
            return Err(ImageError::Malformed("unsafe staging path".into()));
        }
        let checksum = format!("{:x}", Sha256::digest(&decoded));
        outputs.push(GeneratedOutput {
            staging_path: canonical,
            bytes: decoded,
            format,
            width,
            height,
            checksum,
            actual_seed: seeds[index],
        });
    }
    let model_identifier = info
        .and_then(|info| {
            info.get("sd_model_name")
                .or_else(|| info.get("model"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .filter(|value| value.len() <= 256);
    Ok(AdapterResult {
        outputs,
        model_identifier,
    })
}

fn output_seeds(
    info: Option<&serde_json::Value>,
    request: &ImageGenerationRequest,
) -> Result<Vec<i64>, ImageError> {
    if let Some(values) = info
        .and_then(|value| value.get("all_seeds").or_else(|| value.get("allSeeds")))
        .and_then(serde_json::Value::as_array)
    {
        let seeds = values
            .iter()
            .map(|value| value.as_i64().filter(|seed| *seed >= 0))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| ImageError::Malformed("invalid all_seeds metadata".into()))?;
        if seeds.len() != request.output_count as usize {
            return Err(ImageError::Malformed("wrong all_seeds count".into()));
        }
        return Ok(seeds);
    }
    if request.output_count == 1 {
        return Ok(vec![request.seed.unwrap()]);
    }
    Err(ImageError::Malformed(
        "multi-output response is missing all_seeds metadata".into(),
    ))
}

pub fn validate_payload(payload: &ImageJobPayload) -> Result<(), ImageError> {
    if payload.provider_id != PROVIDER_ID
        || payload.provider_version != PROVIDER_VERSION
        || payload.request.seed.is_none()
    {
        return Err(ImageError::InvalidRequest(
            "invalid persisted provider metadata".into(),
        ));
    }
    payload.request.clone().normalize().map(|_| ())
}

pub fn generation_staging(project: &ProjectState, job_id: &str) -> Result<PathBuf, ImageError> {
    let id =
        Uuid::parse_str(job_id).map_err(|_| ImageError::InvalidRequest("invalid job id".into()))?;
    if id.to_string() != job_id {
        return Err(ImageError::InvalidRequest("invalid job id".into()));
    }
    let root = project
        .root
        .join(".nexora")
        .join("jobs")
        .join(job_id)
        .join("generation-staging");
    fs::create_dir_all(&root)?;
    let root = dunce::canonicalize(root)?;
    let project_root = dunce::canonicalize(&project.root)?;
    if !root.starts_with(project_root) {
        return Err(ImageError::InvalidRequest("unsafe staging path".into()));
    }
    Ok(root)
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
    payload: &ImageJobPayload,
    result: &AdapterResult,
) -> Result<bool, ImageError> {
    if !project.execution_enabled() {
        return Ok(false);
    }
    let master_dir = project.root.join("assets").join("masters");
    let preview_dir = project.root.join("assets").join("previews");
    fs::create_dir_all(&master_dir)?;
    fs::create_dir_all(&preview_dir)?;
    let mut files = Vec::new();
    for output in &result.outputs {
        let asset_id = Uuid::now_v7().to_string();
        let extension = if output.format == ImageFormat::Png {
            "png"
        } else {
            "jpg"
        };
        let master = master_dir.join(format!("{asset_id}.{extension}"));
        let preview = preview_dir.join(format!("{asset_id}_preview.png"));
        let master_temp = master_dir.join(format!(".{asset_id}.{extension}.tmp"));
        let preview_temp = preview_dir.join(format!(".{asset_id}_preview.png.tmp"));
        files.push((asset_id, master, preview, master_temp, preview_temp, output));
        let (_, master, preview, master_temp, preview_temp, _) = files.last().unwrap();
        if let Err(error) = fs::copy(&output.staging_path, master_temp) {
            cleanup_promoted_files(&files);
            return Err(error.into());
        }
        let image = image::load_from_memory_with_format(&output.bytes, output.format)
            .map_err(|_| ImageError::Malformed("validated image could not be reopened".into()))?;
        if let Err(error) = image.save_with_format(preview_temp, ImageFormat::Png) {
            cleanup_promoted_files(&files);
            return Err(ImageError::Io(std::io::Error::other(error)));
        }
        if let Err(error) = fs::rename(master_temp, master) {
            cleanup_promoted_files(&files);
            return Err(error.into());
        }
        if let Err(error) = fs::rename(preview_temp, preview) {
            cleanup_promoted_files(&files);
            return Err(error.into());
        }
    }

    // Files must precede their DB rows. A process crash here can leave orphan files; normal
    // failures are cleaned below, while startup orphan reconciliation remains future work.
    let transaction_result = (|| -> Result<bool, ImageError> {
        let mut db = project.db.lock().unwrap();
        if !project.execution_enabled() {
            return Ok(false);
        }
        let tx = db
            .transaction()
            .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        let (status, cancelled): (String, bool) = tx.query_row(
            "SELECT status, cancellation_requested FROM jobs WHERE job_id=?1 AND owner_token=?2",
            rusqlite::params![job_id, owner],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        if status != "running" || cancelled {
            if status == "running" && cancelled {
                let now = now_ms();
                tx.execute("UPDATE jobs SET status='cancelled', updated_at_ms=?1, completed_at_ms=?1, owner_token=NULL WHERE job_id=?2 AND status='running'", rusqlite::params![now, job_id])
                    .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
                tx.execute("INSERT INTO job_events (job_id,event_type,from_status,to_status,message,attempt_count,created_at_ms) SELECT job_id,'cancelled','running','cancelled','output discarded after cancellation',attempt_count,?1 FROM jobs WHERE job_id=?2", rusqlite::params![now, job_id])
                    .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
                tx.commit()
                    .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
            }
            return Ok(false);
        }
        let now = now_ms();
        let settings = serde_json::to_string(&payload.request)?;
        for (index, (asset_id, master, _, _, _, output)) in files.iter().enumerate() {
            let format = if output.format == ImageFormat::Png {
                "PNG"
            } else {
                "JPEG"
            };
            let has_alpha = image::load_from_memory(&output.bytes)
                .map(|image| image.color().has_alpha())
                .unwrap_or(false);
            tx.execute(
                "INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,image_width,image_height,image_format,has_alpha,imported_at_ms,status,source_type,media_kind,media_format,media_width,media_height) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'ready','generated','image',?9,?7,?8)",
                rusqlite::params![asset_id, project.manifest.project_id, format!("generated-{}.{}", index + 1, if output.format == ImageFormat::Png { "png" } else { "jpg" }), master.to_string_lossy(), output.bytes.len() as i64, output.checksum, output.width, output.height, format, has_alpha, now],
            ).map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
            tx.execute(
                "INSERT INTO asset_provenance (asset_id,source_type,provider_id,provider_version,model_identifier,provider_license_state,provider_license_ref,model_license_state,model_license_ref,commercial_use_allowed,prompt,negative_prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) VALUES (?1,'generated',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1,?13,?14,?15)",
                rusqlite::params![asset_id, payload.provider_id, payload.provider_version, result.model_identifier, format!("{:?}", payload.provider_license.status).to_ascii_lowercase(), payload.provider_license.source_reference, payload.provider_license.model_license.as_ref().map(|_| "known"), payload.provider_license.model_license, payload.provider_license.commercial_use_allowed, payload.request.prompt, payload.request.negative_prompt, output.actual_seed, settings, job_id, now],
            ).map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        }
        tx.execute("UPDATE jobs SET status='completed',updated_at_ms=?1,completed_at_ms=?1,progress=100,owner_token=NULL WHERE job_id=?2 AND status='running' AND owner_token=?3", rusqlite::params![now, job_id, owner])
            .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        tx.execute("INSERT INTO job_events (job_id,event_type,from_status,to_status,attempt_count,created_at_ms) SELECT job_id,'completed','running','completed',attempt_count,?1 FROM jobs WHERE job_id=?2", rusqlite::params![now, job_id])
            .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        tx.commit()
            .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
        Ok(true)
    })();
    if !matches!(transaction_result, Ok(true)) {
        for (_, master, preview, master_temp, preview_temp, _) in &files {
            let _ = fs::remove_file(master);
            let _ = fs::remove_file(preview);
            let _ = fs::remove_file(master_temp);
            let _ = fs::remove_file(preview_temp);
        }
    }
    transaction_result
}

fn cleanup_promoted_files(
    files: &[(String, PathBuf, PathBuf, PathBuf, PathBuf, &GeneratedOutput)],
) {
    for (_, master, preview, master_temp, preview_temp, _) in files {
        let _ = fs::remove_file(master);
        let _ = fs::remove_file(preview);
        let _ = fs::remove_file(master_temp);
        let _ = fs::remove_file(preview_temp);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationResult {
    pub job_id: String,
    pub status: String,
    pub asset_ids: Vec<String>,
}

pub fn get_result(
    project: &ProjectState,
    job_id: &str,
) -> Result<ImageGenerationResult, ImageError> {
    Uuid::parse_str(job_id).map_err(|_| ImageError::InvalidRequest("invalid job id".into()))?;
    let db = project.db.lock().unwrap();
    let status: String = db
        .query_row(
            "SELECT status FROM jobs WHERE job_id=?1 AND job_type='image.generate'",
            [job_id],
            |row| row.get(0),
        )
        .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
    let mut statement = db
        .prepare(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1 ORDER BY asset_id",
        )
        .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
    let asset_ids = statement
        .query_map([job_id], |row| row.get(0))
        .map_err(|error| ImageError::InvalidRequest(error.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|error| ImageError::InvalidRequest(error.to_string()))?;
    Ok(ImageGenerationResult {
        job_id: job_id.into(),
        status,
        asset_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{LicenseStatus, ProviderRegistry};
    use image::{DynamicImage, ImageBuffer, Rgb, Rgba};
    use std::{
        io::{Cursor, Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };
    use tempfile::tempdir;

    fn project() -> (tempfile::TempDir, ProjectState) {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Images").unwrap();
        (dir, project)
    }

    fn request(output_count: u32) -> ImageGenerationRequest {
        ImageGenerationRequest {
            schema_version: 1,
            prompt: "fixture".into(),
            negative_prompt: None,
            width: 256,
            height: 256,
            seed: Some(7),
            steps: 10,
            guidance: 7.0,
            output_count,
        }
    }

    fn encoded(format: ImageFormat) -> String {
        let image = if format == ImageFormat::Png {
            DynamicImage::ImageRgba8(ImageBuffer::<Rgba<u8>, _>::from_pixel(
                256,
                256,
                Rgba([1, 2, 3, 255]),
            ))
        } else {
            DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, _>::from_pixel(
                256,
                256,
                Rgb([1, 2, 3]),
            ))
        };
        let mut cursor = Cursor::new(Vec::new());
        image.write_to(&mut cursor, format).unwrap();
        base64::prelude::BASE64_STANDARD.encode(cursor.into_inner())
    }

    fn payload() -> ImageJobPayload {
        ImageJobPayload {
            provider_id: PROVIDER_ID.into(),
            provider_version: PROVIDER_VERSION.into(),
            provider_license: LicenseMetadata {
                status: LicenseStatus::Unknown,
                name: None,
                model_license: None,
                commercial_use_allowed: None,
                source_reference: None,
            },
            request: request(1),
        }
    }

    fn http_fixture(response: Vec<u8>) -> (String, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                let header_end = request.windows(4).position(|value| value == b"\r\n\r\n");
                if let Some(header_end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let _ = sender.send(request);
            stream.write_all(&response).unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    #[test]
    fn config_is_versioned_persistent_and_strictly_loopback() {
        let directory = tempdir().unwrap();
        let path = directory
            .path()
            .join("config")
            .join("image-provider.v1.json");
        let mut config = load_config(&path).unwrap();
        config.enabled = true;
        config.base_url = "http://[::1]:8188".into();
        save_config(&path, &config).unwrap();
        assert_eq!(load_config(&path).unwrap(), config);
        for invalid in [
            "https://127.0.0.1:7860",
            "http://127.0.0.1",
            "http://evil.test:7860",
            "http://localhost:7860/path",
            "http://user@localhost:7860",
            "http://localhost:7860?x=1",
            "http://127.0.0.1.evil.test:7860",
            "http://localhost:7860",
        ] {
            let mut bad = config.clone();
            bad.base_url = invalid.into();
            assert!(bad.validate().is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn request_bounds_unknown_fields_and_random_seed_normalization() {
        let mut value = serde_json::to_value(request(1)).unwrap();
        value["command"] = "evil".into();
        assert!(serde_json::from_value::<ImageGenerationRequest>(value).is_err());
        let mut valid = request(2);
        valid.seed = None;
        assert!(valid.normalize().unwrap().seed.is_some());
        for invalid in [request(0), request(3)] {
            assert!(invalid.normalize().is_err());
        }
        let mut invalid = request(1);
        invalid.width = 257;
        assert!(invalid.normalize().is_err());
        let mut invalid = request(1);
        invalid.guidance = f32::NAN;
        assert!(invalid.normalize().is_err());
    }

    #[test]
    fn png_and_jpeg_fixtures_are_validated_with_controlled_paths() {
        let (_dir, project) = project();
        for (index, format) in [ImageFormat::Png, ImageFormat::Jpeg]
            .into_iter()
            .enumerate()
        {
            let id = Uuid::now_v7().to_string();
            let body = serde_json::to_vec(&serde_json::json!({
                "images": [encoded(format)],
                "info": "{\"sd_model_name\":\"fixture-model\"}"
            }))
            .unwrap();
            let result = parse_response(&project, &id, &request(1), &body).unwrap();
            assert_eq!(result.outputs[0].format, format);
            assert_eq!(result.model_identifier.as_deref(), Some("fixture-model"));
            assert_eq!(
                result.outputs[0].staging_path.file_stem().unwrap(),
                "output-0"
            );
            assert!(
                result.outputs[0]
                    .staging_path
                    .starts_with(generation_staging(&project, &id).unwrap())
            );
            assert_eq!(index, if format == ImageFormat::Png { 0 } else { 1 });
        }
    }

    #[test]
    fn multi_output_seeds_are_exact_and_missing_metadata_is_rejected() {
        let (_dir, project) = project();
        let body = serde_json::to_vec(&serde_json::json!({
            "images": [encoded(ImageFormat::Png), encoded(ImageFormat::Png)],
            "info": {"all_seeds": [101, 202]}
        }))
        .unwrap();
        let result =
            parse_response(&project, &Uuid::now_v7().to_string(), &request(2), &body).unwrap();
        assert_eq!(
            result
                .outputs
                .iter()
                .map(|output| output.actual_seed)
                .collect::<Vec<_>>(),
            vec![101, 202]
        );

        let body = serde_json::to_vec(&serde_json::json!({
            "images": [encoded(ImageFormat::Png), encoded(ImageFormat::Png)]
        }))
        .unwrap();
        assert!(matches!(
            parse_response(&project, &Uuid::now_v7().to_string(), &request(2), &body),
            Err(ImageError::Malformed(message)) if message.contains("all_seeds")
        ));
    }

    #[test]
    fn malformed_corrupt_zero_and_wrong_count_outputs_are_permanent() {
        let (_dir, project) = project();
        for images in [
            serde_json::json!([]),
            serde_json::json!([""]),
            serde_json::json!([base64::prelude::BASE64_STANDARD.encode(b"not-image")]),
            serde_json::json!([encoded(ImageFormat::Png), encoded(ImageFormat::Png)]),
        ] {
            let body = serde_json::to_vec(&serde_json::json!({"images": images})).unwrap();
            let error = parse_response(&project, &Uuid::now_v7().to_string(), &request(1), &body)
                .unwrap_err();
            assert!(!error.retryable());
        }
    }

    #[test]
    fn http_status_and_transport_classes_control_retry() {
        assert!(matches!(
            classify_status(StatusCode::SERVICE_UNAVAILABLE),
            Err(ImageError::Server(503))
        ));
        assert!(matches!(
            classify_status(StatusCode::BAD_REQUEST),
            Err(ImageError::Rejected(400))
        ));
        assert!(classify_status(StatusCode::OK).is_ok());
        assert!(matches!(
            classify_status(StatusCode::REQUEST_TIMEOUT),
            Err(ImageError::Server(408))
        ));
        assert!(matches!(
            classify_status(StatusCode::TOO_MANY_REQUESTS),
            Err(ImageError::Server(429))
        ));
        assert!(
            classify_body_io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)).retryable()
        );
        assert!(ImageError::Timeout.retryable());
        assert!(ImageError::Unavailable("offline".into()).retryable());
        assert!(!ImageError::Malformed("bad image".into()).retryable());
    }

    #[test]
    fn real_adapter_maps_request_and_refuses_redirects() {
        let body = serde_json::to_vec(&serde_json::json!({
            "images": [encoded(ImageFormat::Png)],
            "info": {"all_seeds": [7]}
        }))
        .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes()
        .into_iter()
        .chain(body)
        .collect();
        let (base_url, captured) = http_fixture(response);
        let config = ImageProviderConfig {
            enabled: true,
            base_url,
            ..ImageProviderConfig::default()
        };
        let bytes = execute_http(&payload(), &config).unwrap();
        assert!(!bytes.is_empty());
        let request = String::from_utf8(captured.recv().unwrap()).unwrap();
        assert!(request.starts_with("POST /sdapi/v1/txt2img HTTP/1.1"));
        let json: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(json["batch_size"], 1);
        assert_eq!(json["seed"], 7);
        assert_eq!(json["save_images"], false);

        let (base_url, _) = http_fixture(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\n\r\n".to_vec(),
        );
        let config = ImageProviderConfig {
            enabled: true,
            base_url,
            ..ImageProviderConfig::default()
        };
        let redirect = execute_http(&payload(), &config);
        assert!(
            matches!(redirect, Err(ImageError::Rejected(302))),
            "unexpected redirect result: {redirect:?}"
        );
    }

    #[test]
    fn promotion_is_atomic_immutable_and_records_provenance() {
        let (_dir, project) = project();
        let registry = ProviderRegistry::phase6(true).unwrap();
        let mut registry = registry;
        registry
            .set_local_a1111_state(true, crate::providers::HealthState::Healthy, None)
            .unwrap();
        let created = crate::jobs::create_image_generation(
            &project,
            &registry,
            &crate::hardware::HardwareSnapshot::unknown(),
            crate::jobs::CreateImageGenerationJobInput {
                request: request(1),
                provider_id: None,
            },
        )
        .unwrap();
        let command_result = serde_json::to_value(&created).unwrap();
        assert!(command_result.get("compatibility").is_some());
        assert!(command_result.get("fitWarning").is_none());
        let job_id = created.job.job_id;
        project.with_db(|db| db.execute("UPDATE jobs SET status='running',owner_token='worker',attempt_count=1 WHERE job_id=?1", [&job_id]).map(|_| ())).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({"images": [encoded(ImageFormat::Png)], "info": {"model":"fixture"}})).unwrap();
        let result = parse_response(&project, &job_id, &request(1), &body).unwrap();
        assert!(promote(&project, &job_id, "worker", &payload(), &result).unwrap());
        let generated = get_result(&project, &job_id).unwrap();
        assert_eq!(
            (generated.status.as_str(), generated.asset_ids.len()),
            ("completed", 1)
        );
        let (source, checksum, path, provenance): (String, String, String, i64) = project.with_db(|db| db.query_row(
            "SELECT a.source_type,a.checksum,a.managed_master_path,(SELECT COUNT(*) FROM asset_provenance p WHERE p.asset_id=a.asset_id AND p.actual_seed=7 AND p.provider_id='local.a1111') FROM assets a WHERE a.asset_id=?1",
            [&generated.asset_ids[0]], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap();
        assert_eq!((source.as_str(), provenance), ("generated", 1));
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
            checksum
        );
        assert!(
            project
                .root
                .join("assets/previews")
                .join(format!("{}_preview.png", generated.asset_ids[0]))
                .exists()
        );
    }

    #[test]
    fn promotion_persists_each_outputs_actual_seed() {
        let (_dir, project) = project();
        let job_id = Uuid::now_v7().to_string();
        project.with_db(|db| db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts,attempt_count,owner_token) VALUES (?1,'image.generate','running','{}',1,1,2,1,'worker')", [&job_id]).map(|_| ())).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "images": [encoded(ImageFormat::Png), encoded(ImageFormat::Png)],
            "info": {"all_seeds": [101, 202]}
        }))
        .unwrap();
        let request = request(2);
        let result = parse_response(&project, &job_id, &request, &body).unwrap();
        let mut payload = payload();
        payload.request = request;
        assert!(promote(&project, &job_id, "worker", &payload, &result).unwrap());
        let seeds = project
            .with_db(|db| {
                let mut statement = db.prepare(
                    "SELECT actual_seed FROM asset_provenance WHERE generating_job_id=?1 ORDER BY actual_seed",
                )?;
                statement
                    .query_map([&job_id], |row| row.get(0))?
                    .collect::<Result<Vec<i64>, _>>()
            })
            .unwrap();
        assert_eq!(seeds, [101, 202]);
    }

    #[test]
    fn cancellation_before_promotion_discards_files_and_assets() {
        let (_dir, project) = project();
        let job_id = Uuid::now_v7().to_string();
        project.with_db(|db| db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts,attempt_count,owner_token,cancellation_requested) VALUES (?1,'image.generate','running','{}',1,1,2,1,'worker',1)", [&job_id]).map(|_| ())).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({"images": [encoded(ImageFormat::Png)]}))
            .unwrap();
        let result = parse_response(&project, &job_id, &request(1), &body).unwrap();
        assert!(!promote(&project, &job_id, "worker", &payload(), &result).unwrap());
        let (status, assets): (String, i64) = project
            .with_db(|db| {
                Ok((
                    db.query_row(
                        "SELECT status FROM jobs WHERE job_id=?1",
                        [&job_id],
                        |row| row.get(0),
                    )?,
                    db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((status.as_str(), assets), ("cancelled", 0));
    }

    #[test]
    fn promotion_failure_cleans_destination_files_and_registers_no_assets() {
        let (_dir, project) = project();
        let job_id = Uuid::now_v7().to_string();
        project.with_db(|db| db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts,attempt_count,owner_token) VALUES (?1,'image.generate','running','{}',1,1,2,1,'worker')", [&job_id]).map(|_| ())).unwrap();
        let body = serde_json::to_vec(&serde_json::json!({
            "images": [encoded(ImageFormat::Png)],
            "info": {"all_seeds": [7]}
        }))
        .unwrap();
        let result = parse_response(&project, &job_id, &request(1), &body).unwrap();
        let preview_dir = project.root.join("assets/previews");
        fs::create_dir_all(preview_dir.parent().unwrap()).unwrap();
        fs::write(&preview_dir, b"not a directory").unwrap();

        assert!(promote(&project, &job_id, "worker", &payload(), &result).is_err());
        let assets: i64 = project
            .with_db(|db| db.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(assets, 0);
        let masters = project.root.join("assets/masters");
        assert!(
            !masters.exists() || fs::read_dir(masters).unwrap().next().is_none(),
            "failed promotion left destination files"
        );
    }
}

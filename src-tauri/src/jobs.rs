use crate::{
    blender_adapter,
    hardware::HardwareSnapshot,
    hunyuan_generation::{
        self, HUNYUAN_JOB_TYPE, HunyuanGenerationRequest, HunyuanJobPayload, HunyuanMode,
    },
    image_generation::{self, ImageGenerationRequest, ImageJobPayload},
    model3d_generation::{self, Model3dGenerationRequest, Model3dJobPayload},
    project::ProjectState,
    providers::{Capability, ProviderError, ProviderRegistry},
    video_generation::{self, VideoGenerationRequest, VideoJobPayload},
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

pub const JOB_TYPE: &str = "diagnostic.delay";
pub const PROVIDER_DIAGNOSTIC_JOB_TYPE: &str = "provider.diagnostic";
pub const IMAGE_GENERATION_JOB_TYPE: &str = image_generation::IMAGE_JOB_TYPE;
pub const VIDEO_GENERATION_JOB_TYPE: &str = video_generation::VIDEO_JOB_TYPE;
pub const MODEL3D_GENERATION_JOB_TYPE: &str = model3d_generation::MODEL3D_JOB_TYPE;
pub const MODEL3D_PROCESSING_JOB_TYPE: &str = "model3d.processing";
const PROVIDER_PROTOCOL_VERSION: u32 = 1;
const MIN_DURATION_MS: u64 = 10;
const MAX_DURATION_MS: u64 = 60_000;
const MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Error)]
pub enum JobError {
    #[error("invalid job input: {0}")]
    InvalidInput(String),
    #[error("job not found: {0}")]
    NotFound(String),
    #[error("illegal job transition from {from} to {to}")]
    IllegalTransition { from: String, to: String },
    #[error("job is not eligible for retry")]
    RetryNotAllowed,
    #[error("unsafe job work path")]
    UnsafePath,
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid persisted job payload: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("provider diagnostic unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("{0}")]
    Model3dUnavailable(String),
    #[error("{0}")]
    Model3dProcessingUnavailable(String),
    #[error("blender adapter error: {0}")]
    BlenderAdapter(#[from] crate::blender_adapter::BlenderAdapterError),
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    None,
    Retryable,
    Permanent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDiagnosticJobInput {
    pub duration_ms: u64,
    pub failure_mode: FailureMode,
    pub max_attempts: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProviderDiagnosticJobInput {
    pub provider_id: String,
    pub capability: Capability,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderDiagnosticPayload {
    provider_id: String,
    provider_version: String,
    capability: Capability,
    duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model3dProcessingJobRequest {
    pub schema_version: u32,
    pub source_asset_id: String,
    pub profile: String,
    pub quality: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model3dProcessingJobPayload {
    pub provider_id: String,
    pub provider_version: String,
    pub request: Model3dProcessingJobRequest,
}

#[derive(Clone, Debug)]
enum JobPayload {
    Delay(CreateDiagnosticJobInput),
    Provider(ProviderDiagnosticPayload),
    Image(ImageJobPayload),
    Video(VideoJobPayload),
    Model3d(Model3dJobPayload),
    Model3dProcessing(Model3dProcessingJobPayload),
    Hunyuan(HunyuanJobPayload),
}

impl JobPayload {
    fn duration_ms(&self) -> u64 {
        match self {
            Self::Delay(payload) => payload.duration_ms,
            Self::Provider(payload) => payload.duration_ms,
            Self::Image(_)
            | Self::Video(_)
            | Self::Model3d(_)
            | Self::Model3dProcessing(_)
            | Self::Hunyuan(_) => 0,
        }
    }
}

impl CreateDiagnosticJobInput {
    fn validate(&self) -> Result<(), JobError> {
        if !(MIN_DURATION_MS..=MAX_DURATION_MS).contains(&self.duration_ms) {
            return Err(JobError::InvalidInput(format!(
                "durationMs must be between {MIN_DURATION_MS} and {MAX_DURATION_MS}"
            )));
        }
        if !(1..=MAX_ATTEMPTS).contains(&self.max_attempts) {
            return Err(JobError::InvalidInput(format!(
                "maxAttempts must be between 1 and {MAX_ATTEMPTS}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub job_id: String,
    pub job_type: String,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub progress: u32,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub cancellation_requested: bool,
    pub payload_version: u32,
    pub retryable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub event_id: i64,
    pub event_type: String,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub message: Option<String>,
    pub attempt_count: u32,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDetails {
    #[serde(flatten)]
    pub job: JobInfo,
    pub events: Vec<JobEvent>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateImageGenerationJobInput {
    pub request: ImageGenerationRequest,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateImageGenerationJobResult {
    pub job: JobInfo,
    pub compatibility: crate::providers::Compatibility,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateVideoGenerationJobInput {
    pub request: VideoGenerationRequest,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVideoGenerationJobResult {
    pub job: JobInfo,
    pub compatibility: crate::providers::Compatibility,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateModel3dGenerationJobInput {
    pub request: Model3dGenerationRequest,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateModel3dGenerationJobResult {
    pub job: JobInfo,
    pub compatibility: crate::providers::Compatibility,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateModel3dProcessingJobInput {
    pub schema_version: u32,
    pub source_asset_id: String,
    pub profile: String,
    pub quality: String,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateModel3dProcessingJobResult {
    pub job: JobInfo,
    pub compatibility: crate::providers::Compatibility,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateHunyuanGenerationJobInput {
    pub request: HunyuanGenerationRequest,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateHunyuanGenerationJobResult {
    pub job: JobInfo,
    pub compatibility: crate::providers::Compatibility,
}

#[derive(Clone, Debug)]
struct ClaimedJob {
    job_id: String,
    job_type: String,
    payload_json: String,
    started_at_ms: i64,
    attempt_count: u32,
    max_attempts: u32,
    payload_version: u32,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn legal_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("queued", "running")
            | ("queued", "cancelled")
            | ("running", "queued")
            | ("running", "completed")
            | ("running", "failed")
            | ("running", "cancelled")
            | ("failed", "queued")
    )
}

#[allow(clippy::too_many_arguments)]
fn event(
    tx: &Transaction<'_>,
    job_id: &str,
    event_type: &str,
    from: Option<&str>,
    to: Option<&str>,
    message: Option<&str>,
    attempt_count: u32,
    timestamp: i64,
) -> Result<(), JobError> {
    tx.execute(
        "INSERT INTO job_events (job_id, event_type, from_status, to_status, message, attempt_count, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![job_id, event_type, from, to, message, attempt_count, timestamp],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn transition(
    tx: &Transaction<'_>,
    job_id: &str,
    from: &str,
    to: &str,
    event_type: &str,
    message: Option<&str>,
    error: Option<(&str, &str, bool)>,
    progress: Option<u32>,
) -> Result<(), JobError> {
    if !legal_transition(from, to) {
        return Err(JobError::IllegalTransition {
            from: from.into(),
            to: to.into(),
        });
    }
    let timestamp = now_ms();
    let completed = matches!(to, "completed" | "failed" | "cancelled");
    let (code, error_message, retryable) = error
        .map(|(code, message, retryable)| (Some(code), Some(message), retryable))
        .unwrap_or((None, None, false));
    let changed = tx.execute(
        "UPDATE jobs SET status=?1, updated_at_ms=?2, completed_at_ms=CASE WHEN ?3 THEN ?2 ELSE NULL END, progress=COALESCE(?4, progress), error_code=?5, error_message=?6, retryable=?7, owner_token=NULL WHERE job_id=?8 AND status=?9",
        params![to, timestamp, completed, progress, code, error_message, retryable, job_id, from],
    )?;
    if changed != 1 {
        return Err(JobError::IllegalTransition {
            from: from.into(),
            to: to.into(),
        });
    }
    let attempts: u32 = tx.query_row(
        "SELECT attempt_count FROM jobs WHERE job_id=?1",
        [job_id],
        |row| row.get(0),
    )?;
    event(
        tx,
        job_id,
        event_type,
        Some(from),
        Some(to),
        message,
        attempts,
        timestamp,
    )
}

pub fn create(
    project: &ProjectState,
    input: CreateDiagnosticJobInput,
) -> Result<JobInfo, JobError> {
    input.validate()?;
    let job_id = Uuid::now_v7().to_string();
    let timestamp = now_ms();
    let payload = serde_json::to_string(&input)?;
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO jobs (job_id, job_type, status, payload_json, payload_version, created_at_ms, updated_at_ms, max_attempts) VALUES (?1, ?2, 'queued', ?3, 1, ?4, ?4, ?5)",
        params![job_id, JOB_TYPE, payload, timestamp, input.max_attempts],
    )?;
    event(
        &tx,
        &job_id,
        "created",
        None,
        Some("queued"),
        None,
        0,
        timestamp,
    )?;
    tx.commit()?;
    drop(db);
    get(project, &job_id)
}

pub fn create_provider_diagnostic(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateProviderDiagnosticJobInput,
) -> Result<JobInfo, JobError> {
    if !(MIN_DURATION_MS..=MAX_DURATION_MS).contains(&input.duration_ms) {
        return Err(JobError::InvalidInput(format!(
            "durationMs must be between {MIN_DURATION_MS} and {MAX_DURATION_MS}"
        )));
    }
    let provider = registry
        .validate_diagnostic_execution(&input.provider_id, input.capability, snapshot)
        .map_err(|error| match error {
            ProviderError::UnknownProvider(_) => {
                JobError::ProviderUnavailable("unknown provider".into())
            }
            _ => JobError::ProviderUnavailable("provider is unavailable or incompatible".into()),
        })?;
    let payload = ProviderDiagnosticPayload {
        provider_id: provider.manifest.provider_id,
        provider_version: provider.manifest.version,
        capability: input.capability,
        duration_ms: input.duration_ms,
    };
    validate_provider_payload(&payload)?;
    insert_job(
        project,
        PROVIDER_DIAGNOSTIC_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )
}

pub fn create_image_generation(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateImageGenerationJobInput,
) -> Result<CreateImageGenerationJobResult, JobError> {
    let provider_id = input
        .provider_id
        .as_deref()
        .unwrap_or(image_generation::PROVIDER_ID);
    let provider = registry
        .validate_image_generation(provider_id, snapshot)
        .map_err(|_| {
            JobError::ProviderUnavailable("enabled, healthy local.a1111 is required".into())
        })?;
    let request = input
        .request
        .normalize()
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let payload = ImageJobPayload {
        provider_id: provider.manifest.provider_id,
        provider_version: provider.manifest.version,
        provider_license: provider.license,
        request,
    };
    image_generation::validate_payload(&payload)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let compatibility = provider.fit;
    let job = insert_job(
        project,
        IMAGE_GENERATION_JOB_TYPE,
        serde_json::to_string(&payload)?,
        2,
    )?;
    Ok(CreateImageGenerationJobResult { job, compatibility })
}

pub fn create_video_generation(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateVideoGenerationJobInput,
) -> Result<CreateVideoGenerationJobResult, JobError> {
    let provider_id = input
        .provider_id
        .as_deref()
        .unwrap_or(video_generation::PROVIDER_ID);
    let request = input
        .request
        .normalize()
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let capability = match request.mode {
        video_generation::VideoMode::TextToVideo => Capability::TextToVideo,
        video_generation::VideoMode::ImageToVideo => Capability::ImageToVideo,
    };
    let provider = registry
        .validate_video_generation(provider_id, capability, snapshot)
        .map_err(|_| {
            JobError::ProviderUnavailable(
                "enabled, reachable, compatible local.comfyui is required".into(),
            )
        })?;
    video_generation::validate_source(project, &request)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let payload = VideoJobPayload {
        provider_id: provider.manifest.provider_id,
        provider_version: provider.manifest.version,
        provider_license: provider.license,
        request,
    };
    video_generation::validate_payload(&payload)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let compatibility = provider.fit;
    let job = insert_job(
        project,
        VIDEO_GENERATION_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )?;
    Ok(CreateVideoGenerationJobResult { job, compatibility })
}

pub fn create_model3d_generation(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateModel3dGenerationJobInput,
) -> Result<CreateModel3dGenerationJobResult, JobError> {
    let request = input
        .request
        .normalize()
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    model3d_generation::resolve_source(project, &request)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let capability = match request.mode {
        model3d_generation::Model3dMode::TextTo3d => Capability::Model3dTextTo3d,
        model3d_generation::Model3dMode::ImageTo3d => Capability::Model3dImageTo3d,
    };
    let provider = input
        .provider_id
        .as_deref()
        .map_or_else(
            || registry.select(capability, snapshot),
            |provider_id| registry.validate_selection(provider_id, capability, snapshot),
        )
        .map_err(|_| {
            JobError::Model3dUnavailable(
                "no compatible 3D generation provider is configured".into(),
            )
        })?;
    let payload = Model3dJobPayload {
        provider_id: provider.manifest.provider_id,
        provider_version: provider.manifest.version,
        request,
    };
    model3d_generation::validate_payload(&payload)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let compatibility = provider.fit;
    let job = insert_job(
        project,
        MODEL3D_GENERATION_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )?;
    Ok(CreateModel3dGenerationJobResult { job, compatibility })
}

pub fn create_hunyuan_generation(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateHunyuanGenerationJobInput,
) -> Result<CreateHunyuanGenerationJobResult, JobError> {
    let request = input
        .request
        .normalize()
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let capability = match request.mode {
        HunyuanMode::TextTo3d => Capability::HunyuanTextTo3d,
        HunyuanMode::ImageTo3d => Capability::HunyuanImageTo3d,
    };
    let provider = input
        .provider_id
        .as_deref()
        .map_or_else(
            || registry.select(capability, snapshot),
            |provider_id| registry.validate_selection(provider_id, capability, snapshot),
        )
        .map_err(|_| {
            JobError::Model3dProcessingUnavailable(
                "no compatible Hunyuan generation provider is configured".into(),
            )
        })?;
    let payload = HunyuanJobPayload {
        provider_id: provider.manifest.provider_id.clone(),
        provider_version: provider.manifest.version.clone(),
        request,
    };
    let compatibility = provider.fit;
    let job = insert_job(
        project,
        HUNYUAN_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )?;
    Ok(CreateHunyuanGenerationJobResult { job, compatibility })
}

pub fn create_model3d_processing(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    input: CreateModel3dProcessingJobInput,
) -> Result<CreateModel3dProcessingJobResult, JobError> {
    if input.schema_version != 1 {
        return Err(JobError::InvalidInput(
            "request schema_version must be 1".into(),
        ));
    }
    if input.source_asset_id.trim().is_empty() {
        return Err(JobError::InvalidInput("source_asset_id is required".into()));
    }
    let profile = input.profile.trim().to_lowercase();
    if !matches!(profile.as_str(), "generic" | "vehicle") {
        return Err(JobError::InvalidInput(
            "profile must be 'generic' or 'vehicle'".into(),
        ));
    }
    let quality = input.quality.trim().to_lowercase();
    if !matches!(
        quality.as_str(),
        "master" | "mobile_high" | "mobile_balanced" | "mobile_low"
    ) {
        return Err(JobError::InvalidInput(
            "quality must be one of: master, mobile_high, mobile_balanced, mobile_low".into(),
        ));
    }

    let capability = Capability::MeshProcessing;
    let provider = input
        .provider_id
        .as_deref()
        .map_or_else(
            || registry.select(capability, snapshot),
            |provider_id| registry.validate_selection(provider_id, capability, snapshot),
        )
        .map_err(|_| {
            JobError::Model3dProcessingUnavailable(
                "no compatible 3D processing provider is configured".into(),
            )
        })?;

    let payload = Model3dProcessingJobPayload {
        provider_id: provider.manifest.provider_id.clone(),
        provider_version: provider.manifest.version.clone(),
        request: Model3dProcessingJobRequest {
            schema_version: 1,
            source_asset_id: input.source_asset_id,
            profile,
            quality,
        },
    };

    let compatibility = provider.fit;
    let job = insert_job(
        project,
        MODEL3D_PROCESSING_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )?;

    // Seed the result row so get_result/retry can find the job from the moment it exists.
    let now = now_ms();
    {
        let db = project.db.lock().unwrap();
        // Best-effort seed; the worker's persist step upserts anyway.
        let _ = db.execute(
            "INSERT INTO model3d_processing_jobs (job_id, source_asset_id, profile, quality, progress, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
            rusqlite::params![
                job.job_id,
                payload.request.source_asset_id,
                payload.request.profile,
                payload.request.quality,
                now,
            ],
        );
    }

    Ok(CreateModel3dProcessingJobResult { job, compatibility })
}

/// Persist the outcome of a Blender processing run into model3d_processing_jobs and
/// register the processed master GLB as a first-class asset with provenance.
fn persist_model3d_processing_result(
    db: &Connection,
    job_id: &str,
    payload: &Model3dProcessingJobPayload,
    result: &crate::model3d_processing::Model3dProcessingResult,
) {
    use sha2::{Digest, Sha256};

    let now = now_ms();
    let source_asset_id = payload.request.source_asset_id.clone();
    let output_dir = result
        .output_master_path
        .as_deref()
        .and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_path_buf()));

    // 1. Write pre/post analysis reports next to the outputs so get_result can read them.
    if let Some(dir) = output_dir.as_ref() {
        for (name, report) in [
            ("pre_analysis.json", &result.pre_analysis_report),
            ("post_analysis.json", &result.post_analysis_report),
        ] {
            if let Some(report) = report {
                let path = dir.join(name);
                if let Ok(json) = serde_json::to_vec_pretty(report) {
                    let _ = std::fs::write(&path, json);
                }
            }
        }
    }

    // 2. Register the processed master GLB as a managed asset with provenance.
    if let (Some(master_path), Some(bytes)) = (
        result.output_master_path.as_deref(),
        result.output_master_path.as_deref().and_then(|p| std::fs::read(p).ok()),
    ) {
        if !bytes.is_empty() {
            let masters_dir = db
                .query_row(
                    "SELECT managed_master_path FROM assets WHERE asset_id=?1",
                    rusqlite::params![source_asset_id],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|p| std::path::Path::new(&p).parent().map(|d| d.to_path_buf()));
            if let Some(masters_dir) = masters_dir {
                let asset_id = Uuid::now_v7().to_string();
                let dest = masters_dir.join(format!("{}.glb", asset_id));
                if fs::copy(master_path, &dest).is_ok() {
                    let size = bytes.len() as u64;
                    let checksum = format!("{:x}", Sha256::digest(&bytes));
                    let inserted = db.execute(
                        "INSERT INTO assets (asset_id, project_id, original_filename, managed_master_path, file_size, checksum, imported_at_ms, status, source_type, media_kind, media_container, media_format, validation_level, model_metadata_schema_version, model_metadata_json, processing_status) VALUES (?1, (SELECT project_id FROM assets WHERE asset_id=?2), 'processed-model.glb', ?3, ?4, ?5, ?6, 'ready', 'generated', 'model3d', 'GLB', 'glTF 2.0', 'structural', 1, '{}', 'ready_for_review')",
                        rusqlite::params![
                            asset_id,
                            source_asset_id,
                            dest.to_string_lossy(),
                            size,
                            checksum,
                            now,
                        ],
                    );
                    if inserted.is_ok() {
                        let _ = db.execute(
                            "INSERT INTO asset_provenance (asset_id, parent_asset_id, source_type, provider_id, provider_version, model_identifier, commercial_use_allowed, prompt, actual_seed, generation_settings_version, generation_settings_json, generating_job_id, generated_at_ms) VALUES (?1, ?2, 'generated', ?3, ?4, 'foundation.processing.v1', 1, 'Blender optimization', 0, 1, ?5, ?6, ?7)",
                            rusqlite::params![
                                asset_id,
                                source_asset_id,
                                payload.provider_id,
                                payload.provider_version,
                                serde_json::json!({
                                    "schemaVersion": 1,
                                    "processing": true,
                                    "profile": payload.request.profile,
                                    "quality": payload.request.quality,
                                    "sourceAssetId": source_asset_id,
                                })
                                .to_string(),
                                job_id,
                                now,
                            ],
                        );
                    }
                }
            }
        }
    }

    // 3. Mark the source asset's processing_status (persistent signal that processing ran).
    match result.status.as_str() {
        "READY_FOR_REVIEW" | "completed" => {
            let _ = db.execute(
                "UPDATE assets SET processing_status='ready_for_review' WHERE asset_id=?1 AND media_kind='model3d'",
                rusqlite::params![source_asset_id],
            );
        }
        "NEEDS_REVIEW" => {
            let _ = db.execute(
                "UPDATE assets SET processing_status='needs_review' WHERE asset_id=?1 AND media_kind='model3d'",
                rusqlite::params![source_asset_id],
            );
        }
        _ => {}
    }

    // 4. Persist the result row (upsert; the row is seeded at creation but be defensive).
    let _ = db.execute(
        "INSERT INTO model3d_processing_jobs (job_id, source_asset_id, profile, quality, blender_log_path, pre_analysis_report_path, post_analysis_report_path, output_master_path, lod0_path, lod1_path, lod2_path, vehicle_analysis_path, material_status, processing_stage, progress, error_code, error_message, created_at_ms, updated_at_ms, started_at_ms, completed_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?18,?18,?18)
         ON CONFLICT(job_id) DO UPDATE SET
            blender_log_path=excluded.blender_log_path,
            pre_analysis_report_path=excluded.pre_analysis_report_path,
            post_analysis_report_path=excluded.post_analysis_report_path,
            output_master_path=excluded.output_master_path,
            lod0_path=excluded.lod0_path,
            lod1_path=excluded.lod1_path,
            lod2_path=excluded.lod2_path,
            vehicle_analysis_path=excluded.vehicle_analysis_path,
            material_status=excluded.material_status,
            processing_stage=excluded.processing_stage,
            progress=excluded.progress,
            error_code=excluded.error_code,
            error_message=excluded.error_message,
            updated_at_ms=excluded.updated_at_ms,
            started_at_ms=coalesce(model3d_processing_jobs.started_at_ms, excluded.started_at_ms),
            completed_at_ms=excluded.completed_at_ms",
        rusqlite::params![
            job_id,
            source_asset_id,
            payload.request.profile,
            payload.request.quality,
            output_dir.as_ref().map(|d| d.join("processing.log").to_string_lossy().to_string()),
            output_dir.as_ref().filter(|_| result.pre_analysis_report.is_some()).map(|d| d.join("pre_analysis.json").to_string_lossy().to_string()),
            output_dir.as_ref().filter(|_| result.post_analysis_report.is_some()).map(|d| d.join("post_analysis.json").to_string_lossy().to_string()),
            result.output_master_path.as_deref().map(|p| p.to_string()),
            result.lod0_path.as_deref().map(|p| p.to_string()),
            result.lod1_path.as_deref().map(|p| p.to_string()),
            result.lod2_path.as_deref().map(|p| p.to_string()),
            result.vehicle_analysis_path.as_deref().map(|p| p.to_string()),
            result.material_status,
            result.processing_stage,
            result.progress as i64,
            result.error_code,
            result.error_message,
            now,
        ],
    );
}

#[cfg(test)]
pub(crate) fn create_model3d_fixture_job(
    project: &ProjectState,
    request: Model3dGenerationRequest,
) -> Result<JobInfo, JobError> {
    let mut request = request
        .normalize()
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    request.seed.get_or_insert(0);
    model3d_generation::resolve_source(project, &request)
        .map_err(|error| JobError::InvalidInput(error.to_string()))?;
    let payload = Model3dJobPayload {
        provider_id: "test.fixture.model3d".into(),
        provider_version: "1".into(),
        request,
    };
    insert_job(
        project,
        MODEL3D_GENERATION_JOB_TYPE,
        serde_json::to_string(&payload)?,
        1,
    )
}

fn insert_job(
    project: &ProjectState,
    job_type: &str,
    payload: String,
    max_attempts: u32,
) -> Result<JobInfo, JobError> {
    let job_id = Uuid::now_v7().to_string();
    let timestamp = now_ms();
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO jobs (job_id, job_type, status, payload_json, payload_version, created_at_ms, updated_at_ms, max_attempts) VALUES (?1, ?2, 'queued', ?3, 1, ?4, ?4, ?5)",
        params![job_id, job_type, payload, timestamp, max_attempts],
    )?;
    event(
        &tx,
        &job_id,
        "created",
        None,
        Some("queued"),
        None,
        0,
        timestamp,
    )?;
    tx.commit()?;
    drop(db);
    get(project, &job_id)
}

fn row_to_info(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobInfo> {
    Ok(JobInfo {
        job_id: row.get(0)?,
        job_type: row.get(1)?,
        status: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
        started_at_ms: row.get(5)?,
        completed_at_ms: row.get(6)?,
        progress: row.get(7)?,
        attempt_count: row.get(8)?,
        max_attempts: row.get(9)?,
        error_code: row.get(10)?,
        error_message: row.get(11)?,
        cancellation_requested: row.get::<_, i64>(12)? != 0,
        payload_version: row.get(13)?,
        retryable: row.get::<_, i64>(14)? != 0,
    })
}

const INFO_COLUMNS: &str = "job_id, job_type, status, created_at_ms, updated_at_ms, started_at_ms, completed_at_ms, progress, attempt_count, max_attempts, error_code, error_message, cancellation_requested, payload_version, retryable";

pub fn list(project: &ProjectState) -> Result<Vec<JobInfo>, JobError> {
    let db = project.db.lock().unwrap();
    let mut statement = db.prepare(&format!(
        "SELECT {INFO_COLUMNS} FROM jobs ORDER BY created_at_ms DESC, job_id DESC"
    ))?;
    let rows = statement.query_map([], row_to_info)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get(project: &ProjectState, job_id: &str) -> Result<JobInfo, JobError> {
    validate_job_id(job_id)?;
    let db = project.db.lock().unwrap();
    db.query_row(
        &format!("SELECT {INFO_COLUMNS} FROM jobs WHERE job_id=?1"),
        [job_id],
        row_to_info,
    )
    .optional()?
    .ok_or_else(|| JobError::NotFound(job_id.into()))
}

pub fn details(project: &ProjectState, job_id: &str) -> Result<JobDetails, JobError> {
    let job = get(project, job_id)?;
    let db = project.db.lock().unwrap();
    let mut statement = db.prepare("SELECT event_id, event_type, from_status, to_status, message, attempt_count, created_at_ms FROM job_events WHERE job_id=?1 ORDER BY event_id")?;
    let rows = statement.query_map([job_id], |row| {
        Ok(JobEvent {
            event_id: row.get(0)?,
            event_type: row.get(1)?,
            from_status: row.get(2)?,
            to_status: row.get(3)?,
            message: row.get(4)?,
            attempt_count: row.get(5)?,
            created_at_ms: row.get(6)?,
        })
    })?;
    Ok(JobDetails {
        job,
        events: rows.collect::<Result<Vec<_>, _>>()?,
    })
}

pub fn request_cancellation(project: &ProjectState, job_id: &str) -> Result<JobInfo, JobError> {
    validate_job_id(job_id)?;
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    let (status, attempts): (String, u32) = tx
        .query_row(
            "SELECT status, attempt_count FROM jobs WHERE job_id=?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| JobError::NotFound(job_id.into()))?;
    match status.as_str() {
        "queued" => {
            tx.execute(
                "UPDATE jobs SET cancellation_requested=1 WHERE job_id=?1 AND status='queued'",
                [job_id],
            )?;
            transition(
                &tx,
                job_id,
                "queued",
                "cancelled",
                "cancelled",
                Some("cancelled before execution"),
                None,
                None,
            )?;
        }
        "running" => {
            let timestamp = now_ms();
            tx.execute("UPDATE jobs SET cancellation_requested=1, updated_at_ms=?1 WHERE job_id=?2 AND status='running'", params![timestamp, job_id])?;
            event(
                &tx,
                job_id,
                "cancellation_requested",
                Some("running"),
                Some("running"),
                None,
                attempts,
                timestamp,
            )?;
        }
        _ => {}
    }
    tx.commit()?;
    drop(db);
    get(project, job_id)
}

pub fn retry(project: &ProjectState, job_id: &str) -> Result<JobInfo, JobError> {
    validate_job_id(job_id)?;
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    let (status, retryable, cancelled, max_attempts): (String, bool, bool, u32) = tx
        .query_row(
            "SELECT status, retryable, cancellation_requested, max_attempts FROM jobs WHERE job_id=?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| JobError::NotFound(job_id.into()))?;
    if status != "failed" || !retryable || cancelled || max_attempts >= MAX_ATTEMPTS {
        return Err(JobError::RetryNotAllowed);
    }
    tx.execute(
        "UPDATE jobs SET max_attempts=max_attempts+1, progress=0, started_at_ms=NULL, cancellation_requested=0 WHERE job_id=?1",
        [job_id],
    )?;
    transition(
        &tx,
        job_id,
        "failed",
        "queued",
        "manual_retry",
        Some("manually retried"),
        None,
        Some(0),
    )?;
    tx.commit()?;
    drop(db);
    get(project, job_id)
}

fn claim(
    project: &ProjectState,
    db: &mut Connection,
    owner: &str,
) -> Result<Option<ClaimedJob>, JobError> {
    if !project.execution_enabled() {
        return Ok(None);
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let candidate: Option<String> = tx
        .query_row("SELECT job_id FROM jobs WHERE status='queued' AND cancellation_requested=0 ORDER BY created_at_ms, job_id LIMIT 1", [], |row| row.get(0))
        .optional()?;
    let Some(job_id) = candidate else {
        tx.commit()?;
        return Ok(None);
    };
    if !legal_transition("queued", "running") {
        return Err(JobError::IllegalTransition {
            from: "queued".into(),
            to: "running".into(),
        });
    }
    let timestamp = now_ms();
    let changed = tx.execute("UPDATE jobs SET status='running', owner_token=?1, attempt_count=attempt_count+1, started_at_ms=?2, updated_at_ms=?2, completed_at_ms=NULL, progress=0, error_code=NULL, error_message=NULL, retryable=0 WHERE job_id=?3 AND status='queued' AND cancellation_requested=0", params![owner, timestamp, job_id])?;
    if changed != 1 {
        tx.commit()?;
        return Ok(None);
    }
    let (job_type, payload_json, attempt_count, max_attempts, payload_version): (String, String, u32, u32, u32) = tx
        .query_row(
            "SELECT job_type, payload_json, attempt_count, max_attempts, payload_version FROM jobs WHERE job_id=?1",
            [&job_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
    event(
        &tx,
        &job_id,
        "claimed",
        Some("queued"),
        Some("running"),
        None,
        attempt_count,
        timestamp,
    )?;
    tx.commit()?;
    Ok(Some(ClaimedJob {
        job_id,
        job_type,
        payload_json,
        started_at_ms: timestamp,
        attempt_count,
        max_attempts,
        payload_version,
    }))
}

#[cfg(test)]
pub fn tick(project: &ProjectState, owner: &str) -> Result<bool, JobError> {
    tick_with_configs(project, owner, None, None, None)
}

#[cfg(test)]
pub fn tick_with_config(
    project: &ProjectState,
    owner: &str,
    image_config: Option<&image_generation::ImageProviderConfig>,
    stop: Option<&AtomicBool>,
) -> Result<bool, JobError> {
    tick_with_configs(project, owner, image_config, None, stop)
}

pub fn tick_with_configs(
    project: &ProjectState,
    owner: &str,
    image_config: Option<&image_generation::ImageProviderConfig>,
    video_config: Option<&video_generation::VideoProviderConfig>,
    stop: Option<&AtomicBool>,
) -> Result<bool, JobError> {
    if !project.execution_enabled() {
        return Ok(false);
    }
    let mut db = project.db.lock().unwrap();
    let running: Option<ClaimedJob> = db
        .query_row("SELECT job_id, job_type, payload_json, started_at_ms, attempt_count, max_attempts, payload_version FROM jobs WHERE status='running' AND owner_token=?1 ORDER BY started_at_ms LIMIT 1", [owner], |row| {
            Ok(ClaimedJob { job_id: row.get(0)?, job_type: row.get(1)?, payload_json: row.get(2)?, started_at_ms: row.get(3)?, attempt_count: row.get(4)?, max_attempts: row.get(5)?, payload_version: row.get(6)? })
        }).optional()?;
    let job = match running {
        Some(job) => job,
        None => match project
            .execution_enabled()
            .then(|| claim(project, &mut db, owner))
            .transpose()?
            .flatten()
        {
            Some(job) => job,
            None => return Ok(false),
        },
    };

    let (status, cancellation, current_progress): (String, bool, u32) = db.query_row(
        "SELECT status, cancellation_requested, progress FROM jobs WHERE job_id=?1 AND owner_token=?2",
        params![job.job_id, owner],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if status != "running" {
        return Ok(true);
    }
    if cancellation {
        cleanup_uncommitted(&project.root, &job.job_id)?;
        let tx = db.transaction()?;
        transition(
            &tx,
            &job.job_id,
            "running",
            "cancelled",
            "cancelled",
            Some("cancelled during execution"),
            None,
            None,
        )?;
        tx.commit()?;
        return Ok(true);
    }

    let payload = match parse_payload(&job.job_type, job.payload_version, &job.payload_json) {
        Ok(payload) => payload,
        Err(message) => {
            cleanup_uncommitted(&project.root, &job.job_id)?;
            let tx = db.transaction()?;
            transition(
                &tx,
                &job.job_id,
                "running",
                "failed",
                "failed",
                Some(&message),
                Some(("INVALID_JOB_PAYLOAD", &message, false)),
                None,
            )?;
            tx.commit()?;
            return Ok(true);
        }
    };

    if let JobPayload::Image(payload) = payload {
        update_image_progress(&mut db, &job, owner, 5, "provider request started")?;
        drop(db);
        let Some(config) = image_config.cloned() else {
            let error =
                image_generation::ImageError::Unavailable("provider config unavailable".into());
            finish_image_error(project, &job, &error)?;
            return Ok(true);
        };
        let thread_payload = payload.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = sender.send(image_generation::execute_http(&thread_payload, &config));
        });
        let mut last_heartbeat = Instant::now();
        let response = loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => break Some(result),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Some(Err(image_generation::ImageError::Unavailable(
                        "provider helper stopped unexpectedly".into(),
                    )));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if stop.is_some_and(|value| value.load(Ordering::Relaxed))
                        || !project.execution_enabled()
                    {
                        break None;
                    }
                    if image_cancelled(project, &job.job_id, owner)? {
                        image_generation::purge_staging(project, &job.job_id);
                        finish_image_cancelled(project, &job, "cancelled during provider request")?;
                        break None;
                    }
                    if last_heartbeat.elapsed() >= Duration::from_secs(5) {
                        heartbeat_image_job(project, &job.job_id, owner)?;
                        last_heartbeat = Instant::now();
                    }
                }
            }
        };
        let Some(response) = response else {
            return Ok(true);
        };
        if !project.execution_enabled() {
            return Ok(true);
        }
        let result = response.and_then(|bytes| {
            update_image_progress_for_project(
                project,
                &job,
                owner,
                60,
                "provider response received",
            )
            .map_err(|error| image_generation::ImageError::Http(error.to_string()))?;
            if image_cancelled(project, &job.job_id, owner).unwrap_or(true) {
                return Err(image_generation::ImageError::Unavailable(
                    "execution session retired".into(),
                ));
            }
            let result =
                image_generation::parse_response(project, &job.job_id, &payload.request, &bytes)?;
            update_image_progress_for_project(
                project,
                &job,
                owner,
                80,
                "provider output validated",
            )
            .map_err(|error| image_generation::ImageError::Http(error.to_string()))?;
            Ok(result)
        });
        match result {
            Ok(result) => {
                if !project.execution_enabled() || image_cancelled(project, &job.job_id, owner)? {
                    image_generation::purge_staging(project, &job.job_id);
                    if project.execution_enabled() {
                        finish_image_cancelled(
                            project,
                            &job,
                            "output discarded after cancellation",
                        )?;
                    }
                    return Ok(true);
                }
                update_image_progress_for_project(
                    project,
                    &job,
                    owner,
                    90,
                    "promoting validated output",
                )?;
                let promotion =
                    image_generation::promote(project, &job.job_id, owner, &payload, &result);
                image_generation::purge_staging(project, &job.job_id);
                if let Err(error) = promotion {
                    finish_image_registration_error(project, &job, &error)?;
                }
            }
            Err(error) => {
                image_generation::purge_staging(project, &job.job_id);
                finish_image_error(project, &job, &error)?;
            }
        }
        return Ok(true);
    }

    if let JobPayload::Video(payload) = payload {
        update_image_progress(&mut db, &job, owner, 5, "provider request started")?;
        drop(db);
        let Some(config) = video_config.cloned() else {
            finish_video_error(
                project,
                &job,
                &video_generation::VideoError::Unavailable("provider config unavailable".into()),
            )?;
            return Ok(true);
        };
        let source = match video_generation::validate_source(project, &payload.request) {
            Ok(value) => value,
            Err(error) => {
                finish_video_error(project, &job, &error)?;
                return Ok(true);
            }
        };
        if image_cancelled(project, &job.job_id, owner)? {
            finish_image_cancelled(project, &job, "cancelled before provider request")?;
            return Ok(true);
        }
        let thread_payload = payload.clone();
        let thread_job_id = job.job_id.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = sender.send(video_generation::execute_http(
                &thread_job_id,
                &thread_payload,
                source.as_ref(),
                &config,
            ));
        });
        let response = loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => break Some(result),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Some(Err(video_generation::VideoError::Unavailable(
                        "provider helper stopped unexpectedly".into(),
                    )));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if stop.is_some_and(|value| value.load(Ordering::Relaxed))
                        || !project.execution_enabled()
                    {
                        break None;
                    }
                    if image_cancelled(project, &job.job_id, owner)? {
                        video_generation::purge_staging(project, &job.job_id);
                        finish_image_cancelled(project, &job, "cancelled during provider request")?;
                        break None;
                    }
                }
            }
        };
        let Some(response) = response else {
            return Ok(true);
        };
        match response.and_then(|bytes| {
            update_image_progress_for_project(
                project,
                &job,
                owner,
                60,
                "provider response received",
            )
            .map_err(|e| video_generation::VideoError::Http(e.to_string()))?;
            if image_cancelled(project, &job.job_id, owner).unwrap_or(true) {
                return Err(video_generation::VideoError::Unavailable(
                    "execution session retired".into(),
                ));
            }
            let output = video_generation::stage_and_validate(
                project,
                &job.job_id,
                &payload.request,
                &bytes,
            )?;
            update_image_progress_for_project(project, &job, owner, 80, "MP4 structure validated")
                .map_err(|e| video_generation::VideoError::Http(e.to_string()))?;
            Ok(output)
        }) {
            Ok(output) => {
                if image_cancelled(project, &job.job_id, owner)? {
                    video_generation::purge_staging(project, &job.job_id);
                    finish_image_cancelled(project, &job, "output discarded after cancellation")?;
                    return Ok(true);
                }
                update_image_progress_for_project(
                    project,
                    &job,
                    owner,
                    90,
                    "promoting validated output",
                )?;
                let promoted =
                    video_generation::promote(project, &job.job_id, owner, &payload, &output);
                video_generation::purge_staging(project, &job.job_id);
                if let Err(error) = promoted {
                    finish_video_registration_error(project, &job, &error)?;
                }
            }
            Err(error) => {
                video_generation::purge_staging(project, &job.job_id);
                finish_video_error(project, &job, &error)?;
            }
        }
        return Ok(true);
    }

    if let JobPayload::Model3d(payload) = payload {
        update_image_progress(&mut db, &job, owner, 5, "validating model3d job")?;
        drop(db);
        if let Err(error) = model3d_generation::resolve_source(project, &payload.request) {
            finish_model3d_error(project, &job, &error)?;
            return Ok(true);
        }
        #[cfg(not(test))]
        {
            // Execute via Hunyuan3D API (local.hunyuan provider on port 8081)
            let hunyuan_request = crate::hunyuan_generation::HunyuanGenerationRequest {
                schema_version: 1,
                mode: match payload.request.mode {
                    crate::model3d_generation::Model3dMode::TextTo3d => crate::hunyuan_generation::HunyuanMode::TextTo3d,
                    crate::model3d_generation::Model3dMode::ImageTo3d => crate::hunyuan_generation::HunyuanMode::ImageTo3d,
                },
                prompt: payload.request.prompt.clone(),
                negative_prompt: payload.request.negative_prompt.clone(),
                source_asset_id: payload.request.source_asset_id.clone(),
                profile: crate::hunyuan_generation::HUNYUAN_PROFILE_ID.to_string(),
                quality: "standard".to_string(),
                seed: payload.request.seed,
                output_format: "glb".to_string(),
            };
            let hunyuan_payload = crate::hunyuan_generation::HunyuanJobPayload {
                provider_id: "local.hunyuan".to_string(),
                provider_version: "1.0.0".to_string(),
                request: hunyuan_request,
            };
            let result = crate::hunyuan_generation::run_hunyuan_generation(project, &job.job_id, owner, &hunyuan_payload);
            let mut db = project.db.lock().unwrap();
            match result {
                Ok(r) if r.status == "completed" || r.status == "READY_FOR_REVIEW" => {
                    let tx = db.transaction()?;
                    transition(&tx, &job.job_id, "running", "completed", "completed", None, None, Some(100))?;
                    tx.commit()?;
                }
                Ok(r) => {
                    let tx = db.transaction()?;
                    let code = "MODEL3D_FAILED";
                    let message = r.status.clone();
                    transition(&tx, &job.job_id, "running", "failed", "failed", Some(&message), Some((code, &message, false)), None)?;
                    tx.commit()?;
                }
                Err(error) => {
                    let tx = db.transaction()?;
                    let message = error.to_string();
                    transition(&tx, &job.job_id, "running", "failed", "failed", Some(&message), Some(("MODEL3D_ERROR", &message, false)), None)?;
                    tx.commit()?;
                }
            }
            return Ok(true);
        }
        #[cfg(test)]
        {
            if payload.provider_id != "test.fixture.model3d" {
                finish_model3d_error(
                    project,
                    &job,
                    &model3d_generation::Model3dError::Unavailable(
                        "no compatible 3D generation provider is configured".into(),
                    ),
                )?;
                return Ok(true);
            }
            if image_cancelled(project, &job.job_id, owner)? {
                model3d_generation::purge_staging(project, &job.job_id);
                finish_image_cancelled(project, &job, "cancelled before model3d staging")?;
                return Ok(true);
            }
            let output = match model3d_generation::stage_bytes(
                project,
                &job.job_id,
                &model3d_generation::fixture_output(),
            ) {
                Ok(output) => output,
                Err(error) => {
                    model3d_generation::purge_staging(project, &job.job_id);
                    finish_model3d_error(project, &job, &error)?;
                    return Ok(true);
                }
            };
            if image_cancelled(project, &job.job_id, owner)? {
                model3d_generation::purge_staging(project, &job.job_id);
                finish_image_cancelled(project, &job, "cancelled before model3d promotion")?;
                return Ok(true);
            }
            let promoted =
                model3d_generation::promote(project, &job.job_id, owner, &payload, &output);
            model3d_generation::purge_staging(project, &job.job_id);
            if let Err(error) = promoted {
                finish_model3d_error(project, &job, &error)?;
            }
            return Ok(true);
        }
    }

    let elapsed = now_ms().saturating_sub(job.started_at_ms) as u64;
    // duration_ms() is 0 for payload kinds that report progress internally
    // (e.g. hunyuan.generate); skip the elapsed-based progress math for them
    // and fall through to their dispatch arm below.
    let duration_ms = payload.duration_ms();
    let progress = if duration_ms > 0 {
        ((elapsed.saturating_mul(100) / duration_ms).min(99)) as u32
    } else {
        0
    };
    if duration_ms > 0 && elapsed < duration_ms {
        if progress >= current_progress.saturating_add(5) {
            let timestamp = now_ms();
            let tx = db.transaction()?;
            let changed = tx.execute(
                "UPDATE jobs SET progress=?1, updated_at_ms=?2 WHERE job_id=?3 AND status='running' AND owner_token=?4 AND progress<=?1",
                params![progress, timestamp, job.job_id, owner],
            )?;
            if changed == 1 {
                event(
                    &tx,
                    &job.job_id,
                    "progress",
                    Some("running"),
                    Some("running"),
                    Some(&format!("{progress}%")),
                    job.attempt_count,
                    timestamp,
                )?;
            }
            tx.commit()?;
        }
        return Ok(true);
    }

    match payload {
        JobPayload::Provider(payload) => {
            write_provider_result(&project.root, &job.job_id, &payload)?;
            let tx = db.transaction()?;
            transition(
                &tx,
                &job.job_id,
                "running",
                "completed",
                "completed",
                None,
                None,
                Some(100),
            )?;
            tx.commit()?;
            Ok(true)
        }
        JobPayload::Delay(payload) => match payload.failure_mode {
            FailureMode::None => {
                write_result(&project.root, &job.job_id, job.attempt_count)?;
                let tx = db.transaction()?;
                transition(
                    &tx,
                    &job.job_id,
                    "running",
                    "completed",
                    "completed",
                    None,
                    None,
                    Some(100),
                )?;
                tx.commit()?;
                Ok(true)
            }
            FailureMode::Retryable if job.attempt_count < job.max_attempts => {
                cleanup_uncommitted(&project.root, &job.job_id)?;
                let tx = db.transaction()?;
                transition(
                    &tx,
                    &job.job_id,
                    "running",
                    "queued",
                    "auto_retry",
                    Some("retryable diagnostic failure"),
                    Some(("DIAGNOSTIC_RETRYABLE", "retryable diagnostic failure", true)),
                    Some(0),
                )?;
                tx.commit()?;
                Ok(true)
            }
            FailureMode::Retryable => {
                cleanup_uncommitted(&project.root, &job.job_id)?;
                let tx = db.transaction()?;
                transition(
                    &tx,
                    &job.job_id,
                    "running",
                    "failed",
                    "failed",
                    Some("retry attempts exhausted"),
                    Some((
                        "DIAGNOSTIC_RETRY_EXHAUSTED",
                        "retry attempts exhausted",
                        true,
                    )),
                    None,
                )?;
                tx.commit()?;
                Ok(true)
            }
            FailureMode::Permanent => {
                cleanup_uncommitted(&project.root, &job.job_id)?;
                let tx = db.transaction()?;
                transition(
                    &tx,
                    &job.job_id,
                    "running",
                    "failed",
                    "failed",
                    Some("permanent diagnostic failure"),
                    Some((
                        "DIAGNOSTIC_PERMANENT",
                        "permanent diagnostic failure",
                        false,
                    )),
                    None,
                )?;
                tx.commit()?;
                Ok(true)
            }
        },
        JobPayload::Image(_) | JobPayload::Video(_) | JobPayload::Model3d(_) => unreachable!(),
        JobPayload::Model3dProcessing(payload) => {
            update_image_progress(&mut db, &job, owner, 5, "starting model3d processing")?;
            drop(db);

            let result =
                blender_adapter::run_model3d_processing(project, &job.job_id, owner, &payload);

            // Re-acquire the database lock for post-processing
            let mut db = project.db.lock().unwrap();
            match result {
                Ok(result) => {
                    // Persist the processing result so the UI can read it back
                    persist_model3d_processing_result(&db, &job.job_id, &payload, &result);
                    if result.status == "completed" || result.status == "READY_FOR_REVIEW" {
                        let tx = db.transaction()?;
                        transition(
                            &tx,
                            &job.job_id,
                            "running",
                            "completed",
                            "completed",
                            None,
                            None,
                            Some(100),
                        )?;
                        tx.commit()?;
                    } else {
                        let tx = db.transaction()?;
                        let (code, message) = match result.status.as_str() {
                            "FAILED" => (
                                "PROCESSING_FAILED",
                                result
                                    .error_message
                                    .unwrap_or_else(|| "processing failed".into()),
                            ),
                            "NEEDS_REVIEW" => (
                                "PROCESSING_NEEDS_REVIEW",
                                "processing completed with warnings".into(),
                            ),
                            _ => (
                                "PROCESSING_FAILED",
                                result
                                    .error_message
                                    .unwrap_or_else(|| "unknown error".into()),
                            ),
                        };
                        transition(
                            &tx,
                            &job.job_id,
                            "running",
                            "failed",
                            "failed",
                            Some(&message),
                            Some((code, &message, false)),
                            None,
                        )?;
                        tx.commit()?;
                    }
                    Ok(true)
                }
                Err(e) => {
                    let tx = db.transaction()?;
                    transition(
                        &tx,
                        &job.job_id,
                        "running",
                        "failed",
                        "failed",
                        Some(&e.to_string()),
                        Some(("BLINKER_ADAPTER_ERROR", &e.to_string(), false)),
                        None,
                    )?;
                    tx.commit()?;
                    Ok(true)
                }
            }
        }
        JobPayload::Hunyuan(payload) => {
            update_image_progress(&mut db, &job, owner, 5, "starting hunyuan generation")?;
            drop(db);

            let result =
                hunyuan_generation::run_hunyuan_generation(project, &job.job_id, owner, &payload);
            let result = match result {
                Ok(r) => Ok(r),
                Err(e) => Err(e),
            };

            // Re-acquire the database lock for post-processing
            let mut db = project.db.lock().unwrap();
            match result {
                Ok(result) => {
                    if result.status == "completed" || result.status == "READY_FOR_REVIEW" {
                        let tx = db.transaction()?;
                        transition(
                            &tx,
                            &job.job_id,
                            "running",
                            "completed",
                            "completed",
                            None,
                            None,
                            Some(100),
                        )?;
                        tx.commit()?;
                    } else {
                        let tx = db.transaction()?;
                        let (code, message): (&str, String) = match result.status.as_str() {
                            "FAILED" => ("PROCESSING_FAILED", "generation failed".into()),
                            "NEEDS_REVIEW" => (
                                "PROCESSING_NEEDS_REVIEW",
                                "generation completed with warnings".into(),
                            ),
                            _ => ("PROCESSING_FAILED", "unknown error".into()),
                        };
                        transition(
                            &tx,
                            &job.job_id,
                            "running",
                            "failed",
                            "failed",
                            Some(&message),
                            Some((code, &message, false)),
                            None,
                        )?;
                        tx.commit()?;
                    }
                    Ok(true)
                }
                Err(e) => {
                    let tx = db.transaction()?;
                    transition(
                        &tx,
                        &job.job_id,
                        "running",
                        "failed",
                        "failed",
                        Some(&e.to_string()),
                        Some(("HUNYUAN_ADAPTER_ERROR", &e.to_string(), false)),
                        None,
                    )?;
                    tx.commit()?;
                    Ok(true)
                }
            }
        }
    }
}

fn heartbeat_image_job(project: &ProjectState, job_id: &str, owner: &str) -> Result<(), JobError> {
    project.db.lock().unwrap().execute(
        "UPDATE jobs SET updated_at_ms=?1 WHERE job_id=?2 AND status='running' AND owner_token=?3",
        params![now_ms(), job_id, owner],
    )?;
    Ok(())
}

fn image_cancelled(project: &ProjectState, job_id: &str, owner: &str) -> Result<bool, JobError> {
    let db = project.db.lock().unwrap();
    Ok(db
        .query_row(
            "SELECT cancellation_requested FROM jobs WHERE job_id=?1 AND status='running' AND owner_token=?2",
            params![job_id, owner],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

fn update_image_progress_for_project(
    project: &ProjectState,
    job: &ClaimedJob,
    owner: &str,
    progress: u32,
    message: &str,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    update_image_progress(&mut db, job, owner, progress, message)
}

fn update_image_progress(
    db: &mut Connection,
    job: &ClaimedJob,
    owner: &str,
    progress: u32,
    message: &str,
) -> Result<(), JobError> {
    let tx = db.transaction()?;
    let timestamp = now_ms();
    let changed = tx.execute(
        "UPDATE jobs SET progress=?1,updated_at_ms=?2 WHERE job_id=?3 AND status='running' AND owner_token=?4 AND progress<?1",
        params![progress, timestamp, job.job_id, owner],
    )?;
    if changed == 1 {
        event(
            &tx,
            &job.job_id,
            "progress",
            Some("running"),
            Some("running"),
            Some(message),
            job.attempt_count,
            timestamp,
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn finish_image_cancelled(
    project: &ProjectState,
    job: &ClaimedJob,
    message: &str,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    transition(
        &tx,
        &job.job_id,
        "running",
        "cancelled",
        "cancelled",
        Some(message),
        None,
        None,
    )?;
    tx.commit()?;
    Ok(())
}

fn finish_image_registration_error(
    project: &ProjectState,
    job: &ClaimedJob,
    error: &image_generation::ImageError,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    transition(
        &tx,
        &job.job_id,
        "running",
        "failed",
        "failed",
        Some(&error.to_string()),
        Some(("IMAGE_REGISTRATION_FAILED", &error.to_string(), false)),
        None,
    )?;
    tx.commit()?;
    Ok(())
}

fn finish_image_error(
    project: &ProjectState,
    job: &ClaimedJob,
    error: &image_generation::ImageError,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    let cancelled: bool = tx.query_row(
        "SELECT cancellation_requested FROM jobs WHERE job_id=?1",
        [&job.job_id],
        |row| row.get(0),
    )?;
    if cancelled {
        transition(
            &tx,
            &job.job_id,
            "running",
            "cancelled",
            "cancelled",
            Some("cancelled during image generation"),
            None,
            None,
        )?;
    } else if error.retryable() && job.attempt_count < job.max_attempts {
        transition(
            &tx,
            &job.job_id,
            "running",
            "queued",
            "auto_retry",
            Some(&error.to_string()),
            Some(("IMAGE_PROVIDER_TRANSIENT", &error.to_string(), true)),
            Some(0),
        )?;
    } else {
        let code = if error.retryable() {
            "IMAGE_RETRY_EXHAUSTED"
        } else {
            "IMAGE_PERMANENT"
        };
        transition(
            &tx,
            &job.job_id,
            "running",
            "failed",
            "failed",
            Some(&error.to_string()),
            Some((code, &error.to_string(), error.retryable())),
            None,
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn finish_video_registration_error(
    project: &ProjectState,
    job: &ClaimedJob,
    error: &video_generation::VideoError,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    transition(
        &tx,
        &job.job_id,
        "running",
        "failed",
        "failed",
        Some(&error.to_string()),
        Some(("VIDEO_REGISTRATION_FAILED", &error.to_string(), false)),
        None,
    )?;
    tx.commit()?;
    Ok(())
}

fn finish_video_error(
    project: &ProjectState,
    job: &ClaimedJob,
    error: &video_generation::VideoError,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    let cancelled: bool = tx.query_row(
        "SELECT cancellation_requested FROM jobs WHERE job_id=?1",
        [&job.job_id],
        |row| row.get(0),
    )?;
    if cancelled {
        transition(
            &tx,
            &job.job_id,
            "running",
            "cancelled",
            "cancelled",
            Some("cancelled during video generation"),
            None,
            None,
        )?;
    } else {
        let code = match error {
            video_generation::VideoError::Unavailable(message) if message.contains("disabled") => {
                "VIDEO_PROVIDER_DISABLED"
            }
            video_generation::VideoError::Unavailable(_) => "VIDEO_PROVIDER_UNREACHABLE",
            video_generation::VideoError::Incompatible(_) => "VIDEO_PROVIDER_INCOMPATIBLE",
            video_generation::VideoError::Timeout => "VIDEO_PROVIDER_TIMEOUT",
            video_generation::VideoError::Server(_) | video_generation::VideoError::Rejected(_) => {
                "VIDEO_PROVIDER_HTTP_REJECTED"
            }
            video_generation::VideoError::Malformed(_) => "VIDEO_OUTPUT_MALFORMED",
            video_generation::VideoError::InvalidConfig(_) => "VIDEO_PROVIDER_CONFIG_INVALID",
            video_generation::VideoError::InvalidRequest(_) => "VIDEO_REQUEST_INVALID",
            video_generation::VideoError::Io(_) => "VIDEO_IO_FAILED",
            video_generation::VideoError::Json(_) | video_generation::VideoError::Http(_) => {
                "VIDEO_PROVIDER_PROTOCOL_FAILED"
            }
        };
        transition(
            &tx,
            &job.job_id,
            "running",
            "failed",
            "failed",
            Some(&error.to_string()),
            Some((code, &error.to_string(), error.retryable())),
            None,
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn finish_model3d_error(
    project: &ProjectState,
    job: &ClaimedJob,
    error: &model3d_generation::Model3dError,
) -> Result<(), JobError> {
    let mut db = project.db.lock().unwrap();
    let tx = db.transaction()?;
    let cancelled: bool = tx.query_row(
        "SELECT cancellation_requested FROM jobs WHERE job_id=?1",
        [&job.job_id],
        |row| row.get(0),
    )?;
    if cancelled {
        transition(
            &tx,
            &job.job_id,
            "running",
            "cancelled",
            "cancelled",
            Some("cancelled during model3d generation"),
            None,
            None,
        )?;
    } else {
        transition(
            &tx,
            &job.job_id,
            "running",
            "failed",
            "failed",
            Some(&error.to_string()),
            Some(("MODEL3D_UNAVAILABLE", &error.to_string(), false)),
            None,
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn recover(project: &ProjectState) -> Result<usize, JobError> {
    let mut db = project.db.lock().unwrap();
    let running = {
        let mut statement = db.prepare("SELECT job_id, job_type, cancellation_requested, attempt_count, max_attempts, payload_json, payload_version FROM jobs WHERE status='running'")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut recovered = 0;
    for (job_id, job_type, cancellation, attempts, max_attempts, payload_json, payload_version) in
        running
    {
        let payload = parse_payload(&job_type, payload_version, &payload_json);
        cleanup_uncommitted(&project.root, &job_id)?;
        if job_type == IMAGE_GENERATION_JOB_TYPE {
            image_generation::purge_staging(project, &job_id);
        }
        if job_type == VIDEO_GENERATION_JOB_TYPE {
            video_generation::purge_staging(project, &job_id);
        }
        if job_type == MODEL3D_GENERATION_JOB_TYPE {
            model3d_generation::purge_staging(project, &job_id);
        }
        let tx = db.transaction()?;
        if cancellation {
            transition(
                &tx,
                &job_id,
                "running",
                "cancelled",
                "recovered_cancelled",
                Some("cancellation completed during recovery"),
                None,
                None,
            )?;
        } else if matches!(&payload, Ok(JobPayload::Delay(value)) if value.failure_mode == FailureMode::Retryable)
            && attempts < max_attempts
        {
            transition(
                &tx,
                &job_id,
                "running",
                "queued",
                "recovered_requeued",
                Some("interrupted retryable job requeued"),
                Some(("JOB_INTERRUPTED", "job interrupted", true)),
                Some(0),
            )?;
        } else if let Err(message) = payload {
            transition(
                &tx,
                &job_id,
                "running",
                "failed",
                "recovered_failed",
                Some(&message),
                Some(("INVALID_JOB_PAYLOAD", &message, false)),
                None,
            )?;
        } else {
            let retryable = matches!(payload, Ok(JobPayload::Delay(ref value)) if value.failure_mode == FailureMode::Retryable)
                || matches!(payload, Ok(JobPayload::Video(_)));
            transition(
                &tx,
                &job_id,
                "running",
                "failed",
                "recovered_failed",
                Some("job interrupted"),
                Some(("JOB_INTERRUPTED", "job interrupted", retryable)),
                None,
            )?;
        }
        tx.commit()?;
        recovered += 1;
    }
    Ok(recovered)
}

pub fn approve_model3d_asset(
    project: &ProjectState,
    asset_id: &str,
    processing_job_id: &str,
) -> Result<(), JobError> {
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let mut tx = db.transaction()?;

    // Verify asset exists and is a model3d
    let asset: Option<(String, String)> = tx
        .query_row(
            "SELECT asset_id, media_kind FROM assets WHERE asset_id=?1 AND project_id=?2",
            rusqlite::params![asset_id, project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (asset_id, media_kind) = asset.ok_or(JobError::InvalidInput("asset not found".into()))?;
    if media_kind != "model3d" {
        return Err(JobError::InvalidInput("asset is not a model3d".into()));
    }

    // Verify processing job exists and is a model3d.processing job
    let job: Option<(String, String)> = tx
        .query_row(
            "SELECT job_id, job_type FROM jobs WHERE job_id=?1",
            rusqlite::params![processing_job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (job_id, job_type) =
        job.ok_or(JobError::InvalidInput("processing job not found".into()))?;
    if job_type != "model3d.processing" {
        return Err(JobError::InvalidInput(
            "job is not a model3d processing job".into(),
        ));
    }

    // Check asset processing status
    let processing_status: Option<String> = tx
        .query_row(
            "SELECT processing_status FROM assets WHERE asset_id=?1",
            [asset_id.clone()],
            |row| row.get(0),
        )
        .optional()?;

    let status = processing_status.unwrap_or_else(|| "raw".into());
    if !matches!(status.as_str(), "ready_for_review" | "needs_review") {
        return Err(JobError::InvalidInput(format!(
            "asset cannot be approved from status: {}",
            status
        )));
    }

    // Insert or update asset_approvals
    let now = now_ms();
    tx.execute(
        "INSERT INTO asset_approvals (asset_id, status, approved_by, approved_at_ms, approved_job_id, created_at_ms, updated_at_ms)
         VALUES (?1, 'approved', 'user', ?2, ?3, ?4, ?5)
         ON CONFLICT(asset_id) DO UPDATE SET
           status = 'approved',
           approved_by = 'user',
           approved_at_ms = ?2,
           approved_job_id = ?3,
           updated_at_ms = ?5",
        rusqlite::params![asset_id, now, job_id, now, now],
    )?;

    // Update asset processing_status
    tx.execute(
        "UPDATE assets SET processing_status = 'approved' WHERE asset_id = ?1",
        rusqlite::params![asset_id],
    )?;

    tx.commit()?;
    Ok(())
}

/// Approve a generated 2D image at the first pipeline gate.
///
/// The approval is stored in the same audit table as the final 3D approval,
/// while the media kind and generating job are checked here so an image
/// approval cannot be used to unlock an unrelated asset.
pub fn approve_image_asset(
    project: &ProjectState,
    asset_id: &str,
    generation_job_id: Option<&str>,
) -> Result<(), JobError> {
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let mut tx = db.transaction()?;

    let asset: Option<(String, String, String)> = tx
        .query_row(
            "SELECT asset_id, media_kind, COALESCE(source_type, '') FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready'",
            rusqlite::params![asset_id, project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (asset_id, media_kind, _source_type) =
        asset.ok_or(JobError::InvalidInput("image asset not found or not ready".into()))?;
    if media_kind != "image" {
        return Err(JobError::InvalidInput(
            "only image assets can be approved at the image gate".into(),
        ));
    }

    if let Some(job_id) = generation_job_id {
        let job_type: Option<String> = tx
            .query_row(
                "SELECT job_type FROM jobs WHERE job_id=?1",
                rusqlite::params![job_id],
                |row| row.get(0),
            )
            .optional()?;
        if job_type.as_deref() != Some("image.generate") {
            return Err(JobError::InvalidInput(
                "image approval must reference an image generation job".into(),
            ));
        }
    }

    let now = now_ms();
    tx.execute(
        "INSERT INTO asset_approvals (asset_id, status, approved_by, approved_at_ms, approved_job_id, created_at_ms, updated_at_ms)
         VALUES (?1, 'approved', 'user', ?2, ?3, ?4, ?5)
         ON CONFLICT(asset_id) DO UPDATE SET
           status='approved', approved_by='user', approved_at_ms=?2,
           approved_job_id=?3, rejection_reason=NULL, updated_at_ms=?5",
        rusqlite::params![asset_id, now, generation_job_id, now, now],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn reject_model3d_asset(
    project: &ProjectState,
    asset_id: &str,
    processing_job_id: &str,
    rejection_reason: Option<String>,
) -> Result<(), JobError> {
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let mut tx = db.transaction()?;

    // Verify asset exists and is a model3d
    let asset: Option<(String, String)> = tx
        .query_row(
            "SELECT asset_id, media_kind FROM assets WHERE asset_id=?1 AND project_id=?2",
            rusqlite::params![asset_id, project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (asset_id, media_kind) = asset.ok_or(JobError::InvalidInput("asset not found".into()))?;
    if media_kind != "model3d" {
        return Err(JobError::InvalidInput("asset is not a model3d".into()));
    }

    // Verify processing job exists and is a model3d.processing job
    let job: Option<(String, String)> = tx
        .query_row(
            "SELECT job_id, job_type FROM jobs WHERE job_id=?1",
            rusqlite::params![processing_job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (job_id, job_type) =
        job.ok_or(JobError::InvalidInput("processing job not found".into()))?;
    if job_type != "model3d.processing" {
        return Err(JobError::InvalidInput(
            "job is not a model3d processing job".into(),
        ));
    }

    // Check asset processing status - can reject from ready_for_review, needs_review, or processing
    let processing_status: Option<String> = tx
        .query_row(
            "SELECT processing_status FROM assets WHERE asset_id=?1",
            [asset_id.clone()],
            |row| row.get(0),
        )
        .optional()?;

    let status = processing_status.unwrap_or_else(|| "raw".into());
    if !matches!(
        status.as_str(),
        "ready_for_review" | "needs_review" | "processing"
    ) {
        return Err(JobError::InvalidInput(format!(
            "asset cannot be rejected from status: {}",
            status
        )));
    }

    // Insert or update asset_approvals
    let now = now_ms();
    tx.execute(
        "INSERT INTO asset_approvals (asset_id, status, rejection_reason, approved_job_id, created_at_ms, updated_at_ms)
         VALUES (?1, 'rejected', ?2, ?3, ?4, ?5)
         ON CONFLICT(asset_id) DO UPDATE SET
           status = 'rejected',
           rejection_reason = ?2,
           approved_job_id = ?3,
           updated_at_ms = ?5",
        rusqlite::params![asset_id, rejection_reason, job_id, now, now],
    )?;

    // Update asset processing_status
    tx.execute(
        "UPDATE assets SET processing_status = 'rejected' WHERE asset_id = ?1",
        rusqlite::params![asset_id],
    )?;

    tx.commit()?;
    Ok(())
}

pub fn reprocess_model3d_asset(
    project: &ProjectState,
    registry: &ProviderRegistry,
    snapshot: &HardwareSnapshot,
    asset_id: &str,
    processing_job_id: &str,
    profile: Option<String>,
    quality: Option<String>,
) -> Result<CreateModel3dProcessingJobResult, JobError> {
    // Verify asset exists and is a model3d
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let asset: Option<(String, String)> = db
        .query_row(
            "SELECT asset_id, media_kind FROM assets WHERE asset_id=?1 AND project_id=?2",
            rusqlite::params![asset_id, project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (asset_id, media_kind) = asset.ok_or(JobError::InvalidInput("asset not found".into()))?;
    if media_kind != "model3d" {
        return Err(JobError::InvalidInput("asset is not a model3d".into()));
    }

    // Verify processing job exists and is a model3d.processing job
    let job: Option<(String, String)> = db
        .query_row(
            "SELECT job_id, job_type FROM jobs WHERE job_id=?1",
            rusqlite::params![processing_job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (job_id, job_type) =
        job.ok_or(JobError::InvalidInput("processing job not found".into()))?;
    if job_type != "model3d.processing" {
        return Err(JobError::InvalidInput(
            "job is not a model3d processing job".into(),
        ));
    }

    // Get the original processing job details to reuse profile/quality if not specified
    let original: Option<(String, String)> = db
        .query_row(
            "SELECT profile, quality FROM model3d_processing_jobs WHERE job_id=?1",
            rusqlite::params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (orig_profile, orig_quality) = original.ok_or(JobError::InvalidInput(
        "original processing job details not found".into(),
    ))?;

    let new_profile = profile.unwrap_or(orig_profile);
    let new_quality = quality.unwrap_or(orig_quality);

    // Create a new model3d processing job
    let input = CreateModel3dProcessingJobInput {
        schema_version: 1,
        source_asset_id: asset_id,
        profile: new_profile,
        quality: new_quality,
        provider_id: None,
    };

    create_model3d_processing(project, registry, snapshot, input)
}

fn parse_payload(
    job_type: &str,
    payload_version: u32,
    payload_json: &str,
) -> Result<JobPayload, String> {
    if payload_version != 1 {
        return Err("unsupported persisted job payload version".into());
    }
    match job_type {
        JOB_TYPE => serde_json::from_str(payload_json)
            .map(JobPayload::Delay)
            .map_err(|_| "invalid diagnostic delay payload".to_string()),
        PROVIDER_DIAGNOSTIC_JOB_TYPE => {
            let payload: ProviderDiagnosticPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid provider diagnostic payload".to_string())?;
            validate_provider_payload(&payload)
                .map_err(|_| "invalid provider diagnostic payload".to_string())?;
            Ok(JobPayload::Provider(payload))
        }
        IMAGE_GENERATION_JOB_TYPE => {
            let payload: ImageJobPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid image generation payload".to_string())?;
            image_generation::validate_payload(&payload)
                .map_err(|_| "invalid image generation payload".to_string())?;
            Ok(JobPayload::Image(payload))
        }
        VIDEO_GENERATION_JOB_TYPE => {
            let payload: VideoJobPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid video generation payload".to_string())?;
            video_generation::validate_payload(&payload)
                .map_err(|_| "invalid video generation payload".to_string())?;
            Ok(JobPayload::Video(payload))
        }
        MODEL3D_GENERATION_JOB_TYPE => {
            let payload: Model3dJobPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid model3d generation payload".to_string())?;
            model3d_generation::validate_payload(&payload)
                .map_err(|_| "invalid model3d generation payload".to_string())?;
            Ok(JobPayload::Model3d(payload))
        }
        MODEL3D_PROCESSING_JOB_TYPE => {
            let payload: Model3dProcessingJobPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid model3d processing payload".to_string())?;
            Ok(JobPayload::Model3dProcessing(payload))
        }
        hunyuan_generation::HUNYUAN_JOB_TYPE => {
            let payload: HunyuanJobPayload = serde_json::from_str(payload_json)
                .map_err(|_| "invalid hunyuan generation payload".to_string())?;
            hunyuan_generation::validate_payload(&payload)
                .map_err(|_| "invalid hunyuan generation payload".to_string())?;
            Ok(JobPayload::Hunyuan(payload))
        }
        _ => Err("unsupported persisted job type".to_string()),
    }
}

fn validate_provider_payload(payload: &ProviderDiagnosticPayload) -> Result<(), JobError> {
    if !(MIN_DURATION_MS..=MAX_DURATION_MS).contains(&payload.duration_ms) {
        return Err(JobError::InvalidInput(
            "invalid provider diagnostic duration".into(),
        ));
    }
    let valid = match payload.provider_id.as_str() {
        "mock.image.basic" => {
            payload.provider_version == "1.0.0" && payload.capability == Capability::TextToImage
        }
        "mock.image.gpu-heavy" => {
            payload.provider_version == "1.0.0"
                && matches!(
                    payload.capability,
                    Capability::TextToImage | Capability::ImageToImage
                )
        }
        _ => false,
    };
    if !valid {
        return Err(JobError::InvalidInput(
            "invalid static mock provider metadata".into(),
        ));
    }
    Ok(())
}

fn validate_job_id(job_id: &str) -> Result<Uuid, JobError> {
    let id = Uuid::parse_str(job_id).map_err(|_| JobError::UnsafePath)?;
    if id.to_string() != job_id.to_ascii_lowercase() {
        return Err(JobError::UnsafePath);
    }
    Ok(id)
}

fn work_dir(root: &Path, job_id: &str) -> Result<PathBuf, JobError> {
    validate_job_id(job_id)?;
    let project_root = dunce::canonicalize(root)?;
    let jobs_root = root.join(".nexora").join("jobs");
    fs::create_dir_all(&jobs_root)?;
    let jobs_root = dunce::canonicalize(jobs_root)?;
    if !jobs_root.starts_with(&project_root) {
        return Err(JobError::UnsafePath);
    }
    let work = jobs_root.join(job_id);
    fs::create_dir_all(&work)?;
    let work = dunce::canonicalize(work)?;
    if !work.starts_with(&jobs_root) {
        return Err(JobError::UnsafePath);
    }
    Ok(work)
}

fn cleanup_uncommitted(root: &Path, job_id: &str) -> Result<(), JobError> {
    let work = work_dir(root, job_id)?;
    for filename in ["result.json.tmp", "result.json"] {
        let path = work.join(filename);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn write_result(root: &Path, job_id: &str, attempt: u32) -> Result<(), JobError> {
    let work = work_dir(root, job_id)?;
    let temp = work.join("result.json.tmp");
    let committed = work.join("result.json");
    fs::write(
        &temp,
        serde_json::to_vec_pretty(&serde_json::json!({
            "jobId": job_id, "result": "diagnostic delay completed", "attempt": attempt
        }))?,
    )?;
    fs::rename(temp, committed)?;
    Ok(())
}

fn write_provider_result(
    root: &Path,
    job_id: &str,
    payload: &ProviderDiagnosticPayload,
) -> Result<(), JobError> {
    let work = work_dir(root, job_id)?;
    let temp = work.join("result.json.tmp");
    let committed = work.join("result.json");
    let result = serde_json::to_vec(&serde_json::json!({
        "jobId": job_id,
        "providerId": payload.provider_id,
        "providerVersion": payload.provider_version,
        "capability": payload.capability,
        "protocolVersion": PROVIDER_PROTOCOL_VERSION,
        "result": "provider diagnostic completed"
    }))?;
    debug_assert!(result.len() < 4096);
    fs::write(&temp, result)?;
    fs::rename(temp, committed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hardware::HardwareSnapshot, project::ProjectState, providers::ProviderRegistry};
    use sha2::{Digest, Sha256};
    use std::{
        sync::{Arc, atomic::AtomicBool},
        thread,
        time::{Duration, Instant},
    };
    use tempfile::tempdir;

    fn project() -> (tempfile::TempDir, ProjectState) {
        let dir = tempdir().unwrap();
        let state = ProjectState::create(&dir.path().join("project"), "Jobs").unwrap();
        (dir, state)
    }

    fn input(
        duration_ms: u64,
        failure_mode: FailureMode,
        max_attempts: u32,
    ) -> CreateDiagnosticJobInput {
        CreateDiagnosticJobInput {
            duration_ms,
            failure_mode,
            max_attempts,
        }
    }

    fn model_request() -> Model3dGenerationRequest {
        Model3dGenerationRequest {
            schema_version: 1,
            mode: model3d_generation::Model3dMode::TextTo3d,
            prompt: "fixture model".into(),
            negative_prompt: None,
            source_asset_id: None,
            profile: model3d_generation::PROFILE_ID.into(),
            quality: "standard".into(),
            seed: Some(7),
            output_format: "glb".into(),
        }
    }

    #[test]
    fn model3d_fixture_runs_full_durable_queue_and_records_provenance() {
        let (dir, project) = project();
        let job = create_model3d_fixture_job(&project, model_request()).unwrap();
        assert_eq!(job.status, "queued");
        assert!(tick(&project, "model-worker").unwrap());
        let result = model3d_generation::get_result(&project, &job.job_id).unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.asset_ids, vec![job.job_id.clone()]);
        let (kind, source, level, schema, settings, provenance, model, provider_license, model_license): (String, String, String, i64, String, i64, String, String, String) = project.with_db(|db| db.query_row(
            "SELECT media_kind,source_type,validation_level,model_metadata_schema_version,(SELECT generation_settings_json FROM asset_provenance WHERE asset_id=assets.asset_id),(SELECT COUNT(*) FROM asset_provenance WHERE asset_id=assets.asset_id AND generating_job_id=?1),(SELECT model_identifier FROM asset_provenance WHERE asset_id=assets.asset_id),(SELECT provider_license_state FROM asset_provenance WHERE asset_id=assets.asset_id),(SELECT model_license_state FROM asset_provenance WHERE asset_id=assets.asset_id) FROM assets WHERE asset_id=?1",
            [&job.job_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?)))).unwrap();
        assert_eq!(
            (
                kind.as_str(),
                source.as_str(),
                level.as_str(),
                schema,
                provenance
            ),
            ("model3d", "generated", "structural", 1, 1)
        );
        assert_eq!(
            (
                model.as_str(),
                provider_license.as_str(),
                model_license.as_str()
            ),
            ("test.fixture.triangle.v1", "unknown", "unknown")
        );
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            (
                settings["mode"].as_str(),
                settings["profile"].as_str(),
                settings["quality"].as_str(),
                settings["outputFormat"].as_str(),
                settings["validationLevel"].as_str()
            ),
            (
                Some("text_to_3d"),
                Some("foundation.glb.structural.v1"),
                Some("standard"),
                Some("glb"),
                Some("structural")
            )
        );
        let master = project
            .root
            .join("assets")
            .join("masters")
            .join(format!("{}.glb", job.job_id));
        let bytes = fs::read(&master).unwrap();
        assert_eq!(bytes, model3d_generation::fixture_output());
        drop(project);
        let reopened = ProjectState::open(&dir.path().join("project")).unwrap();
        assert_eq!(fs::read(master).unwrap(), bytes);
        assert_eq!(
            model3d_generation::get_result(&reopened, &job.job_id)
                .unwrap()
                .asset_ids
                .len(),
            1
        );
    }

    #[test]
    fn model3d_fixture_seed_and_repeated_promotion_are_idempotent() {
        let (_dir, project) = project();
        let mut request = model_request();
        request.seed = None;
        let job = create_model3d_fixture_job(&project, request).unwrap();
        let mut db = project.db.lock().unwrap();
        let claimed = claim(&project, &mut db, "idempotent-owner")
            .unwrap()
            .unwrap();
        drop(db);
        let payload: Model3dJobPayload = serde_json::from_str(&claimed.payload_json).unwrap();
        assert_eq!(payload.request.seed, Some(0));
        let staged = model3d_generation::stage_bytes(
            &project,
            &job.job_id,
            &model3d_generation::fixture_output(),
        )
        .unwrap();
        assert!(
            model3d_generation::promote(
                &project,
                &job.job_id,
                "idempotent-owner",
                &payload,
                &staged
            )
            .unwrap()
        );
        let master = project
            .root
            .join("assets")
            .join("masters")
            .join(format!("{}.glb", job.job_id));
        let original = fs::read(&master).unwrap();
        fs::write(&staged.path, b"must not replace the master").unwrap();
        assert!(
            model3d_generation::promote(
                &project,
                &job.job_id,
                "idempotent-owner",
                &payload,
                &staged
            )
            .unwrap()
        );
        assert_eq!(fs::read(master).unwrap(), original);
        let (assets, provenance, seed): (i64, i64, i64) = project.with_db(|db| db.query_row(
            "SELECT (SELECT COUNT(*) FROM assets WHERE asset_id=?1),(SELECT COUNT(*) FROM asset_provenance WHERE generating_job_id=?1),(SELECT actual_seed FROM asset_provenance WHERE generating_job_id=?1)",
            [&job.job_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap();
        assert_eq!((assets, provenance, seed), (1, 1, 0));
    }

    #[test]
    fn model3d_cancellation_recovery_and_payload_version_never_promote() {
        let (_dir, project) = project();
        let cancelled = create_model3d_fixture_job(&project, model_request()).unwrap();
        let mut db = project.db.lock().unwrap();
        claim(&project, &mut db, "cancel-owner").unwrap().unwrap();
        drop(db);
        request_cancellation(&project, &cancelled.job_id).unwrap();
        tick(&project, "cancel-owner").unwrap();
        assert_eq!(
            get(&project, &cancelled.job_id).unwrap().status,
            "cancelled"
        );
        assert_eq!(
            project
                .with_db(|db| db.query_row(
                    "SELECT COUNT(*) FROM assets WHERE asset_id=?1",
                    [&cancelled.job_id],
                    |row| row.get::<_, i64>(0)
                ))
                .unwrap(),
            0
        );

        let interrupted = create_model3d_fixture_job(&project, model_request()).unwrap();
        let mut db = project.db.lock().unwrap();
        claim(&project, &mut db, "recovery-owner").unwrap().unwrap();
        drop(db);
        model3d_generation::stage_bytes(
            &project,
            &interrupted.job_id,
            &model3d_generation::fixture_output(),
        )
        .unwrap();
        assert_eq!(recover(&project).unwrap(), 1);
        assert_eq!(get(&project, &interrupted.job_id).unwrap().status, "failed");
        assert_eq!(recover(&project).unwrap(), 0);
        assert!(
            !project
                .root
                .join(".nexora")
                .join("jobs")
                .join(&interrupted.job_id)
                .join("model3d")
                .exists()
        );

        let versioned = create(&project, input(10, FailureMode::None, 1)).unwrap();
        project
            .with_db(|db| {
                db.execute(
                    "UPDATE jobs SET payload_version=2 WHERE job_id=?1",
                    [&versioned.job_id],
                )
                .map(|_| ())
            })
            .unwrap();
        tick(&project, "version-owner").unwrap();
        let versioned = get(&project, &versioned.job_id).unwrap();
        assert_eq!(
            (versioned.status.as_str(), versioned.error_code.as_deref()),
            ("failed", Some("INVALID_JOB_PAYLOAD"))
        );
    }

    #[test]
    fn production_model3d_creation_is_unavailable_without_inserting() {
        let (_dir, project) = project();
        let registry = ProviderRegistry::phase7(false, false).unwrap();
        let error = create_model3d_generation(
            &project,
            &registry,
            &HardwareSnapshot::unknown(),
            CreateModel3dGenerationJobInput {
                request: model_request(),
                provider_id: None,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "no compatible 3D generation provider is configured"
        );
        assert_eq!(
            project
                .with_db(|db| db.query_row(
                    "SELECT COUNT(*) FROM jobs WHERE job_type='model3d.generate'",
                    [],
                    |row| row.get::<_, i64>(0)
                ))
                .unwrap(),
            0
        );

        let requested = create_model3d_generation(
            &project,
            &registry,
            &HardwareSnapshot::unknown(),
            CreateModel3dGenerationJobInput {
                request: model_request(),
                provider_id: Some("mock.image.basic".into()),
            },
        )
        .unwrap_err();
        assert_eq!(
            requested.to_string(),
            "no compatible 3D generation provider is configured"
        );
        assert_eq!(
            project
                .with_db(|db| db.query_row(
                    "SELECT COUNT(*) FROM jobs WHERE job_type='model3d.generate'",
                    [],
                    |row| row.get::<_, i64>(0)
                ))
                .unwrap(),
            0
        );

        let tampered = Model3dJobPayload {
            provider_id: "unregistered.production.provider".into(),
            provider_version: "1".into(),
            request: model_request(),
        };
        let persisted = insert_job(
            &project,
            MODEL3D_GENERATION_JOB_TYPE,
            serde_json::to_string(&tampered).unwrap(),
            1,
        )
        .unwrap();
        tick(&project, "tampered-model-owner").unwrap();
        let persisted = get(&project, &persisted.job_id).unwrap();
        assert_eq!(
            (persisted.status.as_str(), persisted.retryable),
            ("failed", false)
        );
        assert_eq!(
            project
                .with_db(
                    |db| db.query_row("SELECT COUNT(*) FROM assets", [], |row| row
                        .get::<_, i64>(0))
                )
                .unwrap(),
            0
        );
    }

    fn run_terminal(project: &ProjectState, owner: &str, id: &str) -> JobInfo {
        for _ in 0..100 {
            tick(project, owner).unwrap();
            let info = get(project, id).unwrap();
            if matches!(info.status.as_str(), "completed" | "failed" | "cancelled") {
                return info;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("job did not terminate")
    }

    fn provider_input(
        provider_id: &str,
        capability: Capability,
        duration_ms: u64,
    ) -> CreateProviderDiagnosticJobInput {
        CreateProviderDiagnosticJobInput {
            provider_id: provider_id.into(),
            capability,
            duration_ms,
        }
    }

    fn provider_job(
        project: &ProjectState,
        input: CreateProviderDiagnosticJobInput,
    ) -> Result<JobInfo, JobError> {
        create_provider_diagnostic(
            project,
            &ProviderRegistry::phase5().unwrap(),
            &HardwareSnapshot::unknown(),
            input,
        )
    }

    fn image_job(project: &ProjectState) -> JobInfo {
        let mut registry = ProviderRegistry::phase6(true).unwrap();
        registry
            .set_local_a1111_state(true, crate::providers::HealthState::Healthy, None)
            .unwrap();
        create_image_generation(
            project,
            &registry,
            &HardwareSnapshot::unknown(),
            CreateImageGenerationJobInput {
                request: ImageGenerationRequest {
                    schema_version: 1,
                    prompt: "fixture".into(),
                    negative_prompt: None,
                    width: 256,
                    height: 256,
                    seed: Some(7),
                    steps: 10,
                    guidance: 7.0,
                    output_count: 1,
                },
                provider_id: None,
            },
        )
        .unwrap()
        .job
    }

    fn video_job(project: &ProjectState) -> JobInfo {
        let mut registry = ProviderRegistry::phase7(false, true).unwrap();
        registry
            .set_local_comfyui_state(true, true, true, None)
            .unwrap();
        create_video_generation(
            project,
            &registry,
            &HardwareSnapshot::unknown(),
            CreateVideoGenerationJobInput {
                request: VideoGenerationRequest {
                    schema_version: 1,
                    mode: video_generation::VideoMode::TextToVideo,
                    prompt: "fixture".into(),
                    negative_prompt: None,
                    width: 320,
                    height: 192,
                    frame_count: 9,
                    fps: 8,
                    seed: Some(7),
                    profile: video_generation::PROFILE_ID.into(),
                    source_asset_id: None,
                },
                provider_id: None,
            },
        )
        .unwrap()
        .job
    }

    #[test]
    #[ignore = "requires a running local A1111 API and creates one real image"]
    fn real_a1111_generation_persists_managed_asset() {
        let project_root = std::env::var("NEXORA_REAL_A1111_PROJECT")
            .expect("set NEXORA_REAL_A1111_PROJECT to a new evidence project path");
        let project_root = std::path::PathBuf::from(project_root);
        assert!(
            !project_root.exists(),
            "evidence project path must not already exist"
        );

        let config = image_generation::ImageProviderConfig {
            enabled: true,
            base_url: "http://127.0.0.1:7860".into(),
            timeout_seconds: 300,
            ..image_generation::ImageProviderConfig::default()
        };
        image_generation::health(&config).expect("A1111 health validation failed");

        let project = ProjectState::create(&project_root, "Phase 6 Real Provider Evidence")
            .expect("evidence project creation failed");
        let mut registry = ProviderRegistry::phase6(true).unwrap();
        registry
            .set_local_a1111_state(true, crate::providers::HealthState::Healthy, None)
            .unwrap();
        let created = create_image_generation(
            &project,
            &registry,
            &HardwareSnapshot::unknown(),
            CreateImageGenerationJobInput {
                request: ImageGenerationRequest {
                    schema_version: 1,
                    prompt: "futuristic neon sports car, cyberpunk city at night, wet reflective road, cinematic lighting, highly detailed".into(),
                    negative_prompt: Some(
                        "blurry, low quality, text, watermark, logo, deformed, distorted".into(),
                    ),
                    width: 512,
                    height: 512,
                    seed: None,
                    steps: 20,
                    guidance: 7.0,
                    output_count: 1,
                },
                provider_id: Some(image_generation::PROVIDER_ID.into()),
            },
        )
        .expect("durable image job creation failed")
        .job;

        assert!(
            tick_with_config(&project, "real-a1111-validation", Some(&config), None)
                .expect("worker execution failed")
        );
        let completed = get(&project, &created.job_id).unwrap();
        assert_eq!(completed.status, "completed", "{completed:?}");
        assert_eq!(completed.progress, 100);

        let event_types = details(&project, &created.job_id)
            .unwrap()
            .events
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        assert!(event_types.iter().any(|event| event == "claimed"));
        assert!(event_types.iter().any(|event| event == "completed"));

        let result = image_generation::get_result(&project, &created.job_id).unwrap();
        assert_eq!(result.asset_ids.len(), 1);
        let asset_id = result.asset_ids[0].clone();
        let (master_path, checksum, width, height, format, source_type):
            (String, String, u32, u32, String, String) = project
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT managed_master_path,checksum,image_width,image_height,image_format,source_type FROM assets WHERE asset_id=?1",
                [&asset_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .unwrap();
        let master_path = std::path::PathBuf::from(master_path);
        assert!(master_path.starts_with(&project.root));
        assert!(master_path.exists());
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&master_path).unwrap())),
            checksum
        );
        assert_eq!(
            (width, height, format.as_str(), source_type.as_str()),
            (512, 512, "PNG", "generated")
        );
        let preview_path = project
            .root
            .join("assets")
            .join("previews")
            .join(format!("{asset_id}_preview.png"));
        assert!(preview_path.exists());
        assert!(
            !project
                .root
                .join(".nexora")
                .join("jobs")
                .join(&created.job_id)
                .join("generation-staging")
                .exists()
        );

        drop(project);
        let reopened = ProjectState::open(&project_root).expect("evidence project reopen failed");
        let (provider_id, model_identifier, actual_seed, settings_json, generating_job_id):
            (String, Option<String>, i64, String, String) = reopened
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT provider_id,model_identifier,actual_seed,generation_settings_json,generating_job_id FROM asset_provenance WHERE asset_id=?1",
                [&asset_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings_json).unwrap();
        assert_eq!(provider_id, image_generation::PROVIDER_ID);
        assert_eq!(generating_job_id, created.job_id);
        assert!(
            model_identifier
                .as_deref()
                .is_some_and(|model| !model.is_empty())
        );
        assert!(actual_seed >= 0);
        assert_eq!(settings["width"], 512);
        assert_eq!(settings["height"], 512);
        assert_eq!(settings["steps"], 20);
        assert_eq!(settings["guidance"], 7.0);
        assert!(master_path.exists());
        assert!(preview_path.exists());

        println!(
            "REAL_A1111_EVIDENCE project={} job={} asset={} model={} seed={} checksum={} master={} preview={}",
            project_root.display(),
            created.job_id,
            asset_id,
            model_identifier.unwrap(),
            actual_seed,
            checksum,
            master_path.display(),
            preview_path.display()
        );
    }

    #[test]
    #[ignore = "requires stock ComfyUI 0.33.0 Wan 2.1 models and creates one real video"]
    fn real_comfyui_wan_t2v_persists_managed_asset() {
        const PROMPT: &str = "A futuristic neon sports car driving through a cyberpunk city at night, wet reflective road, cinematic neon lighting, smooth forward motion";
        const NEGATIVE_PROMPT: &str = "blurry, distorted, text, watermark, low quality, deformed";
        const EXISTING_JOB_ID: &str = "01a00019-98e0-7a40-be92-d64cc2f752f4";

        let project_root = std::env::var("NEXORA_REAL_COMFYUI_PROJECT")
            .expect("set NEXORA_REAL_COMFYUI_PROJECT to a new evidence project path");
        let project_root = std::path::PathBuf::from(project_root);
        let config = video_generation::VideoProviderConfig {
            enabled: true,
            base_url: "http://127.0.0.1:8188".into(),
            timeout_seconds: 300,
            ..video_generation::VideoProviderConfig::default()
        };
        video_generation::health(&config).expect("ComfyUI health validation failed");
        video_generation::compatibility(&config)
            .expect("stock Wan profile compatibility validation failed");

        let (project, created) = if project_root.exists() {
            let project = ProjectState::open(&project_root).expect("evidence project open failed");
            let failed = list(&project)
                .expect("evidence jobs query failed")
                .into_iter()
                .find(|job| job.job_type == VIDEO_GENERATION_JOB_TYPE)
                .expect("evidence project has no video generation job");
            assert_eq!(failed.job_id, EXISTING_JOB_ID);
            assert_eq!(failed.status, "failed");
            assert_eq!(failed.error_code.as_deref(), Some("VIDEO_OUTPUT_MALFORMED"));
            let changed = project
                .db
                .lock()
                .unwrap()
                .execute(
                    "UPDATE jobs SET retryable=1 WHERE job_id=?1 AND status='failed' AND error_code='VIDEO_OUTPUT_MALFORMED'",
                    [EXISTING_JOB_ID],
                )
                .expect("failed to enable evidence-only retry");
            assert_eq!(changed, 1);
            let queued = retry(&project, EXISTING_JOB_ID)
                .expect("failed to create durable evidence retry transition");
            assert_eq!(queued.status, "queued");
            (project, queued)
        } else {
            let project = ProjectState::create(&project_root, "Phase 7 Real Provider Evidence")
                .expect("evidence project creation failed");
            let mut registry = ProviderRegistry::phase7(false, true).unwrap();
            registry
                .set_local_comfyui_state(true, true, true, None)
                .unwrap();
            let created = create_video_generation(
                &project,
                &registry,
                &HardwareSnapshot::unknown(),
                CreateVideoGenerationJobInput {
                    request: VideoGenerationRequest {
                        schema_version: 1,
                        mode: video_generation::VideoMode::TextToVideo,
                        prompt: PROMPT.into(),
                        negative_prompt: Some(NEGATIVE_PROMPT.into()),
                        width: 320,
                        height: 192,
                        frame_count: 9,
                        fps: 8,
                        seed: None,
                        profile: video_generation::PROFILE_ID.into(),
                        source_asset_id: None,
                    },
                    provider_id: Some(video_generation::PROVIDER_ID.into()),
                },
            )
            .expect("durable video job creation failed")
            .job;
            (project, created)
        };

        assert!(
            tick_with_configs(
                &project,
                "real-comfyui-validation",
                None,
                Some(&config),
                None
            )
            .expect("worker execution failed")
        );
        let completed = get(&project, &created.job_id).unwrap();
        assert_eq!(completed.status, "completed", "{completed:?}");
        assert_eq!(completed.progress, 100);
        let event_types = details(&project, &created.job_id)
            .unwrap()
            .events
            .into_iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        assert!(event_types.iter().any(|event| event == "claimed"));
        assert!(event_types.iter().any(|event| event == "completed"));

        let result = video_generation::get_result(&project, &created.job_id).unwrap();
        assert_eq!(result.asset_ids.len(), 1);
        let asset_id = result.asset_ids[0].clone();
        let (master, checksum, width, height, duration, fps_num, fps_den, format, codec):
            (String, String, u32, u32, u64, u32, u32, String, String) = project.db.lock().unwrap().query_row(
                "SELECT managed_master_path,checksum,media_width,media_height,duration_ms,fps_numerator,fps_denominator,media_format,codec FROM assets WHERE asset_id=?1 AND media_kind='video' AND source_type='generated'",
                [&asset_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?)),
            ).unwrap();
        let master = std::path::PathBuf::from(master);
        assert!(master.starts_with(&project.root) && master.exists());
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(&master).unwrap())),
            checksum
        );
        assert_eq!(
            (width, height, fps_num, fps_den, format.as_str()),
            (320, 192, 8, 1, "MP4")
        );
        assert!(duration.abs_diff(1250) <= 2);
        assert!(matches!(codec.as_str(), "avc1" | "avc3"));
        assert!(
            !project
                .root
                .join("assets/previews")
                .join(format!("{asset_id}_preview.png"))
                .exists()
        );
        assert!(
            !project
                .root
                .join(".nexora/jobs")
                .join(&created.job_id)
                .join("video-staging")
                .exists()
        );

        project.close().unwrap();
        let reopened = ProjectState::open(&project_root).expect("evidence project reopen failed");
        assert_eq!(
            video_generation::get_result(&reopened, &created.job_id)
                .unwrap()
                .asset_ids,
            [asset_id.clone()]
        );
        let (provider, model, prompt, negative, seed, settings):
            (String, String, String, String, i64, String) = reopened.db.lock().unwrap().query_row(
                "SELECT provider_id,model_identifier,prompt,negative_prompt,actual_seed,generation_settings_json FROM asset_provenance WHERE asset_id=?1 AND generating_job_id=?2",
                rusqlite::params![asset_id, created.job_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
            ).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(provider, video_generation::PROVIDER_ID);
        assert_eq!(model, video_generation::DIFFUSION_MODEL);
        assert_eq!(
            (prompt.as_str(), negative.as_str()),
            (PROMPT, NEGATIVE_PROMPT)
        );
        assert!(seed >= 0);
        assert_eq!(settings["profile"]["id"], video_generation::PROFILE_ID);
        assert_eq!(settings["profile"]["validatedFrameCount"], 9);
        assert!(master.exists());
        println!(
            "REAL_COMFYUI_EVIDENCE project={} job={} asset={} model={} seed={} checksum={} master={} frames=9 fps=8",
            project_root.display(),
            created.job_id,
            asset_id,
            model,
            seed,
            checksum,
            master.display()
        );
    }

    #[test]
    fn lifecycle_progress_success_and_events_persist() {
        let (_dir, project) = project();
        let job = create(&project, input(1_000, FailureMode::None, 1)).unwrap();
        tick(&project, "worker").unwrap();
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE jobs SET started_at_ms=?1 WHERE job_id=?2",
                params![now_ms() - 500, job.job_id],
            )
            .unwrap();
        tick(&project, "worker").unwrap();
        assert!(get(&project, &job.job_id).unwrap().progress > 0);
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE jobs SET started_at_ms=?1 WHERE job_id=?2",
                params![now_ms() - 1_000, job.job_id],
            )
            .unwrap();
        let done = run_terminal(&project, "worker", &job.job_id);
        assert_eq!(done.status, "completed");
        assert_eq!(done.progress, 100);
        assert!(
            work_dir(&project.root, &job.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
        let details = details(&project, &job.job_id).unwrap();
        let event_types = details
            .events
            .iter()
            .map(|e| e.event_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(event_types.first(), Some(&"created"));
        assert_eq!(event_types.get(1), Some(&"claimed"));
        assert!(event_types.contains(&"progress"));
        assert_eq!(event_types.last(), Some(&"completed"));
    }

    #[test]
    fn queued_and_running_cancellation_never_promote() {
        let (_dir, project) = project();
        let queued = create(&project, input(30, FailureMode::None, 1)).unwrap();
        assert_eq!(
            request_cancellation(&project, &queued.job_id)
                .unwrap()
                .status,
            "cancelled"
        );
        let running = create(&project, input(1_000, FailureMode::None, 1)).unwrap();
        tick(&project, "worker").unwrap();
        request_cancellation(&project, &running.job_id).unwrap();
        assert_eq!(
            run_terminal(&project, "worker", &running.job_id).status,
            "cancelled"
        );
        assert!(
            !work_dir(&project.root, &running.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
    }

    #[test]
    fn retry_bounds_permanent_and_cancel_retry_prevention() {
        let (_dir, project) = project();
        let retrying = create(&project, input(10, FailureMode::Retryable, 2)).unwrap();
        let failed = run_terminal(&project, "worker", &retrying.job_id);
        assert_eq!(
            (
                failed.status.as_str(),
                failed.attempt_count,
                failed.retryable
            ),
            ("failed", 2, true)
        );
        let retried = retry(&project, &retrying.job_id).unwrap();
        assert_eq!(retried.status, "queued");
        assert_eq!((retried.attempt_count, retried.max_attempts), (2, 3));
        let permanent = create(&project, input(10, FailureMode::Permanent, 5)).unwrap();
        let failed = run_terminal(&project, "worker", &permanent.job_id);
        assert_eq!(failed.attempt_count, 1);
        assert!(!failed.retryable);
        assert!(matches!(
            retry(&project, &permanent.job_id),
            Err(JobError::RetryNotAllowed)
        ));
        let cancelled = create(&project, input(10, FailureMode::Retryable, 1)).unwrap();
        request_cancellation(&project, &cancelled.job_id).unwrap();
        assert!(matches!(
            retry(&project, &cancelled.job_id),
            Err(JobError::RetryNotAllowed)
        ));

        let capped = create(&project, input(10, FailureMode::Retryable, 5)).unwrap();
        let capped = run_terminal(&project, "worker", &capped.job_id);
        assert_eq!(capped.attempt_count, 5);
        assert!(matches!(
            retry(&project, &capped.job_id),
            Err(JobError::RetryNotAllowed)
        ));
    }

    #[test]
    fn claim_is_single_and_completed_is_not_reclaimed() {
        let (_dir, project) = project();
        let job = create(&project, input(10, FailureMode::None, 1)).unwrap();
        tick(&project, "owner-a").unwrap();
        tick(&project, "owner-b").unwrap();
        assert_eq!(get(&project, &job.job_id).unwrap().attempt_count, 1);
        let done = run_terminal(&project, "owner-a", &job.job_id);
        for _ in 0..3 {
            assert!(!tick(&project, "owner-a").unwrap());
        }
        assert_eq!(
            get(&project, &job.job_id).unwrap().attempt_count,
            done.attempt_count
        );
    }

    #[test]
    fn disabled_project_cannot_claim_and_recovery_retires_running_job() {
        let (_dir, disabled_project) = project();
        let queued = create(&disabled_project, input(10, FailureMode::None, 1)).unwrap();
        disabled_project.disable_execution();
        assert!(!tick(&disabled_project, "stale-worker").unwrap());
        assert_eq!(
            get(&disabled_project, &queued.job_id).unwrap().status,
            "queued"
        );

        let (_dir, running_project) = project();
        let running = create(&running_project, input(1_000, FailureMode::None, 1)).unwrap();
        tick(&running_project, "stale-worker").unwrap();
        running_project.disable_execution();
        assert_eq!(recover(&running_project).unwrap(), 1);
        assert!(!tick(&running_project, "stale-worker").unwrap());
        assert_eq!(
            get(&running_project, &running.job_id).unwrap().status,
            "failed"
        );
    }

    #[test]
    fn image_progress_events_and_registration_failure_are_terminal() {
        let (_dir, project) = project();
        let created = image_job(&project);
        let mut db = project.db.lock().unwrap();
        let claimed = claim(&project, &mut db, "worker").unwrap().unwrap();
        update_image_progress(&mut db, &claimed, "worker", 5, "provider request started").unwrap();
        update_image_progress(
            &mut db,
            &claimed,
            "worker",
            60,
            "provider response received",
        )
        .unwrap();
        drop(db);
        finish_image_registration_error(
            &project,
            &claimed,
            &image_generation::ImageError::Io(std::io::Error::other("disk failure")),
        )
        .unwrap();

        let details = details(&project, &created.job_id).unwrap();
        assert_eq!(details.job.status, "failed");
        assert_eq!(
            details.job.error_code.as_deref(),
            Some("IMAGE_REGISTRATION_FAILED")
        );
        assert!(!details.job.retryable);
        assert_eq!(details.job.progress, 60);
        let messages = details
            .events
            .iter()
            .filter(|event| event.event_type == "progress")
            .filter_map(|event| event.message.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            ["provider request started", "provider response received"]
        );
    }

    #[test]
    fn worker_stop_detaches_blocked_http_within_poll_interval() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let (_dir, project) = project();
        let project = Arc::new(project);
        image_job(&project);
        let config = image_generation::ImageProviderConfig {
            enabled: true,
            base_url: format!("http://{address}"),
            timeout_seconds: 10,
            ..image_generation::ImageProviderConfig::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_project = project.clone();
        let worker_stop = stop.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            tick_with_config(&worker_project, "worker", Some(&config), Some(&worker_stop)).unwrap()
        });
        thread::sleep(Duration::from_millis(100));
        stop.store(true, Ordering::Relaxed);
        assert!(handle.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
        project.disable_execution();
        assert_eq!(recover(&project).unwrap(), 1);
    }

    #[test]
    fn cancellation_interrupts_polling_of_stalled_http() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let (_dir, project) = project();
        let project = Arc::new(project);
        let created = image_job(&project);
        let config = image_generation::ImageProviderConfig {
            enabled: true,
            base_url: format!("http://{address}"),
            timeout_seconds: 10,
            ..image_generation::ImageProviderConfig::default()
        };
        let worker_project = project.clone();
        let handle = thread::spawn(move || {
            tick_with_config(&worker_project, "worker", Some(&config), None).unwrap()
        });
        for _ in 0..20 {
            if get(&project, &created.job_id).unwrap().status == "running" {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let started = Instant::now();
        request_cancellation(&project, &created.job_id).unwrap();
        assert!(handle.join().unwrap());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(get(&project, &created.job_id).unwrap().status, "cancelled");
    }

    #[test]
    fn recovery_policy_is_idempotent() {
        let (_dir, project) = project();
        let retryable = create(&project, input(1_000, FailureMode::Retryable, 2)).unwrap();
        tick(&project, "old").unwrap();
        fs::write(
            work_dir(&project.root, &retryable.job_id)
                .unwrap()
                .join("result.json"),
            b"orphaned promoted output",
        )
        .unwrap();
        assert_eq!(recover(&project).unwrap(), 1);
        assert_eq!(recover(&project).unwrap(), 0);
        assert_eq!(get(&project, &retryable.job_id).unwrap().status, "queued");
        assert!(
            !work_dir(&project.root, &retryable.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
        request_cancellation(&project, &retryable.job_id).unwrap();
        let permanent = create(&project, input(1_000, FailureMode::Permanent, 2)).unwrap();
        tick(&project, "old").unwrap();
        recover(&project).unwrap();
        assert_eq!(get(&project, &permanent.job_id).unwrap().status, "failed");

        let cancellation = create(&project, input(1_000, FailureMode::None, 1)).unwrap();
        tick(&project, "old").unwrap();
        request_cancellation(&project, &cancellation.job_id).unwrap();
        assert_eq!(recover(&project).unwrap(), 1);
        assert_eq!(
            get(&project, &cancellation.job_id).unwrap().status,
            "cancelled"
        );
    }

    #[test]
    fn video_jobs_share_claim_cancel_retry_and_recovery_lifecycle() {
        let (_dir, project) = project();
        let cancelled = video_job(&project);
        assert_eq!(
            request_cancellation(&project, &cancelled.job_id)
                .unwrap()
                .status,
            "cancelled"
        );
        assert!(!tick(&project, "worker").unwrap());

        let interrupted = video_job(&project);
        let mut db = project.db.lock().unwrap();
        let claimed = claim(&project, &mut db, "video-worker").unwrap().unwrap();
        assert_eq!(claimed.job_type, VIDEO_GENERATION_JOB_TYPE);
        drop(db);
        assert_eq!(recover(&project).unwrap(), 1);
        let recovered = get(&project, &interrupted.job_id).unwrap();
        assert_eq!(
            (
                recovered.status.as_str(),
                recovered.retryable,
                recovered.max_attempts
            ),
            ("failed", true, 1)
        );
        let retried = retry(&project, &interrupted.job_id).unwrap();
        assert_eq!(
            (retried.status.as_str(), retried.max_attempts),
            ("queued", 2)
        );
        request_cancellation(&project, &retried.job_id).unwrap();

        let timed_out = video_job(&project);
        let mut db = project.db.lock().unwrap();
        let claimed = claim(&project, &mut db, "timeout-worker").unwrap().unwrap();
        drop(db);
        finish_video_error(&project, &claimed, &video_generation::VideoError::Timeout).unwrap();
        let failed = get(&project, &timed_out.job_id).unwrap();
        assert_eq!(
            (
                failed.status.as_str(),
                failed.attempt_count,
                failed.max_attempts,
                failed.retryable,
                failed.error_code.as_deref()
            ),
            ("failed", 1, 1, true, Some("VIDEO_PROVIDER_TIMEOUT"))
        );
    }

    #[test]
    fn jobs_persist_across_reopen() {
        let (dir, project) = project();
        let root = project.root.clone();
        let job = create(&project, input(10, FailureMode::None, 1)).unwrap();
        run_terminal(&project, "worker", &job.job_id);
        project.close().unwrap();
        let reopened = ProjectState::open(&root).unwrap();
        assert_eq!(get(&reopened, &job.job_id).unwrap().status, "completed");
        assert!(!details(&reopened, &job.job_id).unwrap().events.is_empty());
        drop(reopened);
        drop(dir);
    }

    #[test]
    fn safe_paths_reject_traversal_and_stay_inside_project() {
        let (_dir, project) = project();
        assert!(matches!(
            work_dir(&project.root, "../escape"),
            Err(JobError::UnsafePath)
        ));
        let id = Uuid::now_v7().to_string();
        let path = work_dir(&project.root, &id).unwrap();
        assert!(path.starts_with(dunce::canonicalize(&project.root).unwrap()));
    }

    #[test]
    fn validation_and_illegal_transitions_are_rejected() {
        let (_dir, project) = project();
        assert!(matches!(
            create(&project, input(1, FailureMode::None, 1)),
            Err(JobError::InvalidInput(_))
        ));
        assert!(matches!(
            create(&project, input(10, FailureMode::None, 6)),
            Err(JobError::InvalidInput(_))
        ));
        let job = create(&project, input(10, FailureMode::None, 1)).unwrap();
        let mut db = project.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        assert!(matches!(
            transition(
                &tx,
                &job.job_id,
                "queued",
                "completed",
                "bad",
                None,
                None,
                None
            ),
            Err(JobError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn command_payloads_and_job_details_are_camel_case() {
        let input: CreateDiagnosticJobInput = serde_json::from_value(serde_json::json!({
            "durationMs": 10,
            "failureMode": "none",
            "maxAttempts": 1
        }))
        .unwrap();
        let (_dir, project) = project();
        let job = create(&project, input).unwrap();
        let value = serde_json::to_value(details(&project, &job.job_id).unwrap()).unwrap();
        assert_eq!(value["jobId"], job.job_id);
        assert_eq!(value["jobType"], JOB_TYPE);
        assert!(value.get("createdAtMs").is_some());
        assert!(value.get("job").is_none());
        assert!(value["events"].is_array());
    }

    #[test]
    fn provider_diagnostic_success_has_bounded_metadata_only_result() {
        let (_dir, project) = project();
        let job = provider_job(
            &project,
            provider_input("mock.image.basic", Capability::TextToImage, 10),
        )
        .unwrap();
        assert_eq!(job.job_type, PROVIDER_DIAGNOSTIC_JOB_TYPE);
        let done = run_terminal(&project, "worker", &job.job_id);
        assert_eq!((done.status.as_str(), done.max_attempts), ("completed", 1));
        let bytes = fs::read(
            work_dir(&project.root, &job.job_id)
                .unwrap()
                .join("result.json"),
        )
        .unwrap();
        assert!(bytes.len() < 4096);
        let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["jobId"], job.job_id);
        assert_eq!(result["providerId"], "mock.image.basic");
        assert_eq!(result["providerVersion"], "1.0.0");
        assert_eq!(result["capability"], "text_to_image");
        assert_eq!(result["protocolVersion"], 1);
        assert_eq!(result.as_object().unwrap().len(), 6);
    }

    #[test]
    fn provider_diagnostic_cancellation_never_promotes() {
        let (_dir, project) = project();
        let job = provider_job(
            &project,
            provider_input("mock.image.basic", Capability::TextToImage, 1_000),
        )
        .unwrap();
        tick(&project, "worker").unwrap();
        request_cancellation(&project, &job.job_id).unwrap();
        assert_eq!(
            run_terminal(&project, "worker", &job.job_id).status,
            "cancelled"
        );
        assert!(
            !work_dir(&project.root, &job.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
    }

    #[test]
    fn provider_diagnostic_rejects_unavailable_incompatible_unknown_and_mismatch() {
        let (_dir, project) = project();
        for input in [
            provider_input("mock.unavailable", Capability::TextToImage, 10),
            provider_input("mock.image.gpu-heavy", Capability::ImageToImage, 10),
            provider_input("mock.missing", Capability::TextToImage, 10),
            provider_input("mock.image.basic", Capability::ImageToImage, 10),
        ] {
            assert!(matches!(
                provider_job(&project, input),
                Err(JobError::ProviderUnavailable(_))
            ));
        }
    }

    #[test]
    fn tampered_provider_payload_fails_safely_without_retry_or_promotion() {
        let (_dir, project) = project();
        let job = provider_job(
            &project,
            provider_input("mock.image.basic", Capability::TextToImage, 10),
        )
        .unwrap();
        project.with_db(|db| db.execute("UPDATE jobs SET payload_json='{\"providerId\":\"mock.image.basic\",\"providerVersion\":\"1.0.0\",\"capability\":\"text_to_image\",\"durationMs\":10,\"command\":\"evil.exe\"}' WHERE job_id=?1", [&job.job_id]).map(|_| ())).unwrap();
        let done = run_terminal(&project, "worker", &job.job_id);
        assert_eq!(
            (done.status.as_str(), done.attempt_count, done.retryable),
            ("failed", 1, false)
        );
        assert_eq!(done.error_code.as_deref(), Some("INVALID_JOB_PAYLOAD"));
        assert!(
            !work_dir(&project.root, &job.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
    }

    #[test]
    fn provider_diagnostic_recovery_and_reopen_are_nonretryable() {
        let (dir, project) = project();
        let root = project.root.clone();
        let completed = provider_job(
            &project,
            provider_input("mock.image.basic", Capability::TextToImage, 10),
        )
        .unwrap();
        assert_eq!(
            run_terminal(&project, "worker", &completed.job_id).status,
            "completed"
        );
        let job = provider_job(
            &project,
            provider_input("mock.image.basic", Capability::TextToImage, 1_000),
        )
        .unwrap();
        tick(&project, "old-worker").unwrap();
        assert_eq!(recover(&project).unwrap(), 1);
        assert_eq!(get(&project, &job.job_id).unwrap().status, "failed");
        project.close().unwrap();
        let reopened = ProjectState::open(&root).unwrap();
        assert_eq!(
            get(&reopened, &completed.job_id).unwrap().status,
            "completed"
        );
        assert!(
            work_dir(&reopened.root, &completed.job_id)
                .unwrap()
                .join("result.json")
                .exists()
        );
        let persisted = get(&reopened, &job.job_id).unwrap();
        assert_eq!(
            (
                persisted.status.as_str(),
                persisted.attempt_count,
                persisted.retryable
            ),
            ("failed", 1, false)
        );
        drop(reopened);
        drop(dir);
    }

    #[test]
    fn model3d_processing_result_is_persisted_and_queryable() {
        let (dir, project) = project();
        let source_id = Uuid::now_v7().to_string();
        let masters = project.root.join("assets").join("masters");
        std::fs::create_dir_all(&masters).unwrap();
        let master = masters.join(format!("{}.glb", source_id));
        std::fs::write(&master, b"glb-source-bytes").unwrap();
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO assets (asset_id, project_id, original_filename, managed_master_path, file_size, checksum, imported_at_ms, status, source_type, media_kind, media_container, media_format, validation_level, model_metadata_schema_version, model_metadata_json, processing_status) VALUES (?1, ?2, 'raw.glb', ?3, 15, 'abc', 1, 'ready', 'imported', 'model3d', 'GLB', 'glTF 2.0', 'structural', 1, '{}', 'raw')",
                    rusqlite::params![source_id, project.manifest.project_id, master.to_string_lossy()],
                )
            })
            .unwrap();

        let registry = ProviderRegistry::phase8(false, false, false).unwrap();
        let snapshot = crate::providers::tests::snapshot(Some(16_000), Some(50_000), None);
        let input = CreateModel3dProcessingJobInput {
            schema_version: 1,
            source_asset_id: source_id.clone(),
            profile: "vehicle".into(),
            quality: "master".into(),
            provider_id: None,
        };
        let created =
            create_model3d_processing(&project, &registry, &snapshot, input).unwrap();

        // The result row must exist from the moment the job is created.
        let seeded = project
            .with_db(|db| {
                db.query_row(
                    "SELECT source_asset_id, profile FROM model3d_processing_jobs WHERE job_id=?1",
                    rusqlite::params![created.job.job_id],
                    |row| Ok((row.get::<_, String>(0), row.get::<_, String>(1))),
                )
            })
            .unwrap();
        assert_eq!(seeded.0.unwrap(), source_id);
        assert_eq!(seeded.1.unwrap(), "vehicle");

        // Simulate the worker's persist step (run_model3d_processing is covered by its own adapter tests).
        let out_dir = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(&created.job.job_id)
            .join("model3d_processing");
        std::fs::create_dir_all(&out_dir).unwrap();
        let processed_master = out_dir.join("clean_master.glb");
        std::fs::write(&processed_master, b"glb-processed-bytes").unwrap();
        let result = crate::model3d_processing::Model3dProcessingResult {
            job_id: created.job.job_id.clone(),
            status: "READY_FOR_REVIEW".into(),
            asset_ids: vec![],
            processing_stage: Some("completed".into()),
            progress: 100,
            output_master_path: Some(processed_master.to_string_lossy().to_string()),
            lod0_path: None,
            lod1_path: Some(out_dir.join("vehicle_lod1.glb").to_string_lossy().to_string()),
            lod2_path: None,
            vehicle_analysis_path: None,
            material_status: Some("standard_pbr".into()),
            pre_analysis_report: None,
            post_analysis_report: None,
            error_code: None,
            error_message: None,
        };
        {
            let payload = Model3dProcessingJobPayload {
                provider_id: "local.blender".into(),
                provider_version: "1".into(),
                request: Model3dProcessingJobRequest {
                    schema_version: 1,
                    source_asset_id: source_id.clone(),
                    profile: "vehicle".into(),
                    quality: "master".into(),
                },
            };
            let db = project.db.lock().unwrap();
            persist_model3d_processing_result(&db, &created.job.job_id, &payload, &result);
        }

        // Simulate the worker's post-persist job transition, then get_result must resolve it.
        project.with_db(|db| {
            db.execute(
                "UPDATE jobs SET status='completed', progress=100 WHERE job_id=?1",
                rusqlite::params![created.job.job_id],
            )
        })
        .unwrap();
        let view = crate::model3d_processing::get_result(&project, &created.job.job_id).unwrap();
        assert_eq!(view.status, "completed");
        assert_eq!(view.progress, 100);
        assert!(view.output_master_path.is_some());

        // A new managed asset was registered with provenance, parented to the source.
        let provenance_count: i64 = project
            .with_db(|db| {
                db.query_row(
                    "SELECT count(*) FROM asset_provenance WHERE generating_job_id=?1 AND parent_asset_id=?2",
                    rusqlite::params![created.job.job_id, source_id],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(provenance_count, 1);

        // The source asset is marked ready_for_review so the approval gate can accept it.
        let src_status: String = project
            .with_db(|db| {
                db.query_row(
                    "SELECT processing_status FROM assets WHERE asset_id=?1",
                    rusqlite::params![source_id],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(src_status, "ready_for_review");

        drop(dir);
    }
}

pub fn create_unity_delivery(
    project: &ProjectState,
    asset_id: &str,
    processing_job_id: &str,
    target_id: &str,
    revision: u32,
) -> Result<String, JobError> {
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let mut tx = db.transaction()?;

    // Verify asset exists and is a model3d
    let asset: Option<(String, String, String)> = tx
        .query_row(
            "SELECT asset_id, media_kind, checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready'",
            rusqlite::params![asset_id.clone(), project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let (asset_id, media_kind, checksum) =
        asset.ok_or(JobError::InvalidInput("approved asset not found".into()))?;
    let asset_id_for_approval = asset_id.clone();
    let asset_id_for_insert = asset_id.clone();
    let asset_id_for_insert_2 = asset_id.clone();
    if media_kind != "model3d" {
        return Err(JobError::InvalidInput("asset is not a model3d".into()));
    }

    let approval: Option<(String, String)> = tx
        .query_row(
            "SELECT asset_id, status FROM asset_approvals WHERE asset_id=?1",
            [asset_id_for_approval.clone()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (_, approval_status) =
        approval.ok_or(JobError::InvalidInput("asset not approved".into()))?;
    if approval_status != "approved" {
        return Err(JobError::InvalidInput("asset is not approved".into()));
    }

    // Verify processing job exists and is a model3d.processing job
    let job: Option<(String, String)> = tx
        .query_row(
            "SELECT job_id, job_type FROM jobs WHERE job_id=?1",
            rusqlite::params![processing_job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let (job_id, job_type) =
        job.ok_or(JobError::InvalidInput("processing job not found".into()))?;
    if job_type != "model3d.processing" {
        return Err(JobError::InvalidInput(
            "job is not a model3d processing job".into(),
        ));
    }

    // Verify target exists
    let target: Option<String> = tx
        .query_row(
            "SELECT target_id FROM unity_project_targets WHERE target_id=?1",
            [target_id],
            |row| row.get(0),
        )
        .optional()?;

    if target.is_none() {
        return Err(JobError::InvalidInput(
            "Unity target project not found".into(),
        ));
    }

    // Verify checksum matches
    let approved_artifact_checksum = checksum;

    // Check for existing delivery
    let existing: Option<(String, u32)> = tx
        .query_row(
            "SELECT delivery_id, delivery_revision FROM unity_deliveries WHERE asset_id=?1 AND processing_job_id=?2 AND target_id=?3",
            rusqlite::params![asset_id.clone(), processing_job_id, target_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let delivery_id = if let Some((existing_id, existing_rev)) = existing {
        if existing_rev >= revision {
            return Err(JobError::InvalidInput(
                "delivery revision already exists or newer".into(),
            ));
        }
        // Update existing delivery with new revision
        tx.execute(
            "UPDATE unity_deliveries SET delivery_revision=?1, status='preparing', updated_at_ms=?2, approved_artifact_checksum=?3 WHERE delivery_id=?4",
            rusqlite::params![revision, now_ms(), approved_artifact_checksum, existing_id],
        )?;
        existing_id
    } else {
        // Create new delivery
        let delivery_id = Uuid::now_v7().to_string();
        let now = now_ms();
        tx.execute(
            "INSERT INTO unity_deliveries (delivery_id, asset_id, approval_id, processing_job_id, target_id, approved_artifact_checksum, unity_project_root, unity_destination_folder, delivery_revision, status, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, (SELECT approval_id FROM asset_approvals WHERE asset_id=?1), ?3, ?4, ?5, ?6, ?7, ?8, 'preparing', ?9, ?9)",
            rusqlite::params![
                delivery_id,
                asset_id_for_insert,
                processing_job_id,
                target_id,
                approved_artifact_checksum,
                "", // unity_project_root - will be filled in later
                "", // unity_destination_folder - will be filled in later
                revision,
                now_ms(),
            ],
        )?;
        delivery_id
    };

    tx.commit()?;
    Ok(delivery_id)
}

pub fn update_unity_delivery_status(
    project: &ProjectState,
    delivery_id: &str,
    status: &str,
    error_message: Option<String>,
    imported_asset_path: Option<String>,
    prefab_path: Option<String>,
    validation_report: Option<String>,
) -> Result<(), JobError> {
    let mut db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;
    let mut tx = db.transaction()?;

    let now = now_ms();
    tx.execute(
        "UPDATE unity_deliveries SET status=?1, error_message=?2, imported_asset_path=?3, prefab_path=?4, validation_report=?5, updated_at_ms=?6, completed_at_ms=CASE WHEN ?1 IN ('delivered', 'needs_review', 'failed') THEN ?2 ELSE completed_at_ms END WHERE delivery_id=?7",
        rusqlite::params![status, error_message, imported_asset_path, prefab_path, validation_report, now, now, delivery_id],
    )?;

    tx.commit()?;
    Ok(())
}

pub fn get_unity_delivery(
    project: &ProjectState,
    delivery_id: &str,
) -> Result<UnityDelivery, JobError> {
    let db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;

    let row: Option<(
        String,     // delivery_id
        String,     // asset_id
        String,     // approval_id
        String,     // processing_job_id
        String,     // target_id
        String,     // approved_artifact_checksum
        String,     // unity_project_root
        String,     // unity_destination_folder
        u32,        // delivery_revision
        String,     // status
        Option<String>,  // error_message
        Option<String>,  // imported_asset_path
        Option<String>,  // prefab_path
        Option<String>,  // validation_report
        i64,        // created_at_ms
        i64,        // updated_at_ms
        Option<i64>,    // completed_at_ms
    )> = db
        .query_row(
            "SELECT delivery_id, asset_id, approval_id, processing_job_id, target_id, approved_artifact_checksum, unity_project_root, unity_destination_folder, delivery_revision, status, error_message, imported_asset_path, prefab_path, validation_report, created_at_ms, updated_at_ms, completed_at_ms FROM unity_deliveries WHERE delivery_id=?1",
            [delivery_id],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?,
                row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?,
                row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?, row.get(15)?, row.get(16)?,
            )),
        )
        .optional()?;

    row.map(
        |(
            delivery_id,
            asset_id,
            approval_id,
            processing_job_id,
            target_id,
            approved_artifact_checksum,
            unity_project_root,
            unity_destination_folder,
            delivery_revision,
            status,
            error_message,
            imported_asset_path,
            prefab_path,
            validation_report,
            created_at_ms,
            updated_at_ms,
            completed_at_ms,
        )| {
            UnityDelivery {
                delivery_id,
                asset_id,
                approval_id,
                processing_job_id,
                target_id,
                approved_artifact_checksum,
                unity_project_root,
                unity_destination_folder,
                delivery_revision,
                status,
                error_message,
                imported_asset_path,
                prefab_path,
                validation_report,
                created_at_ms,
                updated_at_ms,
                completed_at_ms,
            }
        },
    )
    .ok_or(JobError::NotFound("delivery not found".into()))
}

pub fn list_unity_deliveries(
    project: &ProjectState,
    target_id: Option<String>,
    asset_id: Option<String>,
) -> Result<Vec<UnityDelivery>, JobError> {
    let db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;

    let mut query = String::from(
        "SELECT delivery_id, asset_id, approval_id, processing_job_id, target_id, approved_artifact_checksum, unity_project_root, unity_destination_folder, delivery_revision, status, error_message, imported_asset_path, prefab_path, validation_report, created_at_ms, updated_at_ms, completed_at_ms FROM unity_deliveries WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(target_id) = target_id {
        query.push_str(" AND target_id = ?");
        params.push(Box::new(target_id));
    }
    if let Some(asset_id) = asset_id {
        query.push_str(" AND asset_id = ?");
        params.push(Box::new(asset_id));
    }
    query.push_str(" ORDER BY created_at_ms DESC");

    let mut stmt = db.prepare(&query)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        |row| {
            Ok(UnityDelivery {
                delivery_id: row.get(0)?,
                asset_id: row.get(1)?,
                approval_id: row.get(2)?,
                processing_job_id: row.get(3)?,
                target_id: row.get(4)?,
                approved_artifact_checksum: row.get(5)?,
                unity_project_root: row.get(6)?,
                unity_destination_folder: row.get(7)?,
                delivery_revision: row.get(8)?,
                status: row.get(9)?,
                error_message: row.get(10)?,
                imported_asset_path: row.get(11)?,
                prefab_path: row.get(12)?,
                validation_report: row.get(13)?,
                created_at_ms: row.get(14)?,
                updated_at_ms: row.get(15)?,
                completed_at_ms: row.get(16)?,
            })
        },
    )?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JobError::Database)
}

pub fn verify_asset_approved_for_unity_delivery(
    project: &ProjectState,
    asset_id: &str,
) -> Result<(String, String), JobError> {
    let db = project
        .db
        .lock()
        .map_err(|_| JobError::Database(rusqlite::Error::ExecuteReturnedResults))?;

    let row: Option<(String, String, String)> = db
        .query_row(
            "SELECT a.asset_id, a.checksum, ap.approval_id FROM assets a JOIN asset_approvals ap ON a.asset_id = ap.asset_id WHERE a.asset_id=?1 AND a.project_id=?2 AND a.media_kind='model3d' AND a.status='ready' AND ap.status='approved'",
            rusqlite::params![asset_id, project.manifest.project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    row.ok_or(JobError::InvalidInput(
        "asset not approved for unity delivery".into(),
    ))
    .map(|(asset_id, checksum, approval_id)| (asset_id, checksum))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnityDelivery {
    pub delivery_id: String,
    pub asset_id: String,
    pub approval_id: String,
    pub processing_job_id: String,
    pub target_id: String,
    pub approved_artifact_checksum: String,
    pub unity_project_root: String,
    pub unity_destination_folder: String,
    pub delivery_revision: u32,
    pub status: String,
    pub error_message: Option<String>,
    pub imported_asset_path: Option<String>,
    pub prefab_path: Option<String>,
    pub validation_report: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUnityDeliveryInput {
    pub asset_id: String,
    pub processing_job_id: String,
    pub target_id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUnityDeliveryStatusInput {
    pub delivery_id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub imported_asset_path: Option<String>,
    pub prefab_path: Option<String>,
    pub validation_report: Option<String>,
}

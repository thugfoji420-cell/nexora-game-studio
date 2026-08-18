use crate::{model3d_validation, project::ProjectState};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::blocking::Client;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub const HUNYUAN_JOB_TYPE: &str = "hunyuan.generate";
pub const HUNYUAN_PROFILE_ID: &str = "foundation.hunyuan.v1";

const MAX_PROMPT_CHARS: usize = 2000;
const MAX_SOURCE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 64 * 1024 * 1024;

#[cfg(test)]
const FIXTURE_MODEL_IDENTIFIER: &str = "test.fixture.hunyuan.v1";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HunyuanMode {
    #[serde(rename = "text_to_3d")]
    TextTo3d,
    #[serde(rename = "image_to_3d")]
    ImageTo3d,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HunyuanGenerationRequest {
    pub schema_version: u32,
    pub mode: HunyuanMode,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub source_asset_id: Option<String>,
    pub profile: String,
    pub quality: String,
    pub seed: Option<i64>,
    pub output_format: String,
}

impl HunyuanGenerationRequest {
    pub fn normalize(mut self) -> Result<Self, HunyuanError> {
        self.prompt = self.prompt.trim().to_owned();
        self.negative_prompt = self.negative_prompt.map(|value| value.trim().to_owned());
        let source_valid = match self.mode {
            HunyuanMode::TextTo3d => self.source_asset_id.is_none(),
            HunyuanMode::ImageTo3d => self.source_asset_id.as_deref().is_some_and(valid_uuid),
        };
        if self.schema_version != 1
            || !source_valid
            || self.prompt.is_empty()
            || self.prompt.chars().count() > MAX_PROMPT_CHARS
            || self
                .negative_prompt
                .as_ref()
                .is_some_and(|value| value.chars().count() > MAX_PROMPT_CHARS)
            || self.profile != HUNYUAN_PROFILE_ID
            || self.quality != "standard"
            || self.seed.is_some_and(|seed| seed < 0)
            || self.output_format != "glb"
        {
            return Err(HunyuanError::InvalidRequest(
                "request is outside hunyuan generation v1 bounds".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HunyuanJobPayload {
    pub provider_id: String,
    pub provider_version: String,
    pub request: HunyuanGenerationRequest,
}

pub fn validate_payload(payload: &HunyuanJobPayload) -> Result<(), HunyuanError> {
    if payload.provider_id.is_empty()
        || payload.provider_id.len() > 128
        || payload.provider_version.is_empty()
        || payload.provider_version.len() > 64
    {
        return Err(HunyuanError::InvalidRequest(
            "invalid hunyuan provider identity".into(),
        ));
    }
    payload.request.clone().normalize()?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct SourceImage {
    _bytes: Vec<u8>,
}

pub fn resolve_source(
    project: &ProjectState,
    request: &HunyuanGenerationRequest,
) -> Result<Option<SourceImage>, HunyuanError> {
    let Some(id) = request.source_asset_id.as_deref() else {
        return Ok(None);
    };
    let (path, registered_size, registered_checksum): (String, u64, String) = project.db.lock().unwrap().query_row(
        "SELECT managed_master_path,file_size,checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready' AND media_kind='image'",
        rusqlite::params![id, project.manifest.project_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).map_err(|_| HunyuanError::InvalidRequest("sourceAssetId must reference a READY image in the current project".into()))?;
    let canonical = dunce::canonicalize(path)
        .map_err(|_| HunyuanError::InvalidRequest("source image master is missing".into()))?;
    let masters = dunce::canonicalize(project.root.join("assets").join("masters"))?;
    let metadata = fs::metadata(&canonical)?;
    if !canonical.starts_with(&masters)
        || !metadata.is_file()
        || metadata.len() != registered_size
        || metadata.len() == 0
        || metadata.len() > MAX_SOURCE_BYTES
    {
        return Err(HunyuanError::InvalidRequest(
            "source image is not an intact bounded managed master".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(registered_size as usize);
    fs::File::open(&canonical)?
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != registered_size
        || format!("{:x}", Sha256::digest(&bytes)) != registered_checksum.to_ascii_lowercase()
    {
        return Err(HunyuanError::InvalidRequest(
            "source image size or checksum integrity failed".into(),
        ));
    }
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| HunyuanError::InvalidRequest("source image format is not recognized".into()))?
        .into_dimensions()
        .map_err(|_| HunyuanError::InvalidRequest("source image dimensions are invalid".into()))?;
    if width == 0
        || height == 0
        || u64::from(width)
            .checked_mul(u64::from(height))
            .is_none_or(|pixels| pixels > MAX_SOURCE_PIXELS)
    {
        return Err(HunyuanError::InvalidRequest(
            "source image exceeds the 64 megapixel workload bound".into(),
        ));
    }
    image::load_from_memory(&bytes)
        .map_err(|_| HunyuanError::InvalidRequest("source image is not fully decodable".into()))?;
    Ok(Some(SourceImage { _bytes: bytes }))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HunyuanGenerationResult {
    pub job_id: String,
    pub status: String,
    pub asset_ids: Vec<String>,
    pub error_message: Option<String>,
}

pub fn get_result(
    project: &ProjectState,
    job_id: &str,
) -> Result<HunyuanGenerationResult, HunyuanError> {
    if !valid_uuid(job_id) {
        return Err(HunyuanError::InvalidRequest("invalid job id".into()));
    }
    let db = project.db.lock().unwrap();
    let status = db
        .query_row(
            "SELECT status FROM jobs WHERE job_id=?1 AND job_type='hunyuan.generate'",
            [job_id],
            |row| row.get(0),
        )
        .map_err(|e| HunyuanError::Database(e))?;
    let mut statement = db
        .prepare(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1 ORDER BY asset_id",
        )
        .map_err(|e| HunyuanError::Database(e))?;
    let asset_ids = statement
        .query_map([job_id], |row| row.get(0))
        .map_err(|e| HunyuanError::Database(e))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| HunyuanError::Database(e))?;
    Ok(HunyuanGenerationResult {
        job_id: job_id.into(),
        status,
        asset_ids,
        error_message: None,
    })
}

#[cfg(test)]
pub(crate) fn fixture_output() -> Vec<u8> {
    crate::model3d_validation::tests::fixture_glb()
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn database(error: rusqlite::Error) -> HunyuanError {
    HunyuanError::InvalidRequest(error.to_string())
}

pub fn run_hunyuan_generation(
    project: &ProjectState,
    job_id: &str,
    owner: &str,
    payload: &HunyuanJobPayload,
) -> Result<HunyuanGenerationResult, HunyuanError> {
    // Resolve source image if needed
    if let HunyuanMode::ImageTo3d = payload.request.mode {
        resolve_source(project, &payload.request)?;
    }

    // Create job output directory
    let job_output_dir = project
        .root
        .join(".nexora")
        .join("jobs")
        .join(job_id)
        .join("hunyuan_generation");
    fs::create_dir_all(&job_output_dir)?;

    // Get the source asset path for ImageTo3d
    let input_path = if let HunyuanMode::ImageTo3d = payload.request.mode {
        let Some(source_asset_id) = &payload.request.source_asset_id else {
            return Err(HunyuanError::InvalidRequest(
                "source_asset_id required for image_to_3d".into(),
            ));
        };
        let db = project.db.lock().unwrap();
        db.query_row(
            "SELECT managed_master_path FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready' AND media_kind='image'",
            rusqlite::params![source_asset_id, project.manifest.project_id],
            |row| row.get::<_, String>(0),
        ).map_err(|_| HunyuanError::InvalidRequest("source asset not found".into()))?
    } else {
        String::new() // TextTo3d doesn't need source image
    };

    let input_path_buf = if !input_path.is_empty() {
        Some(PathBuf::from(input_path))
    } else {
        None
    };

    // Call Hunyuan API
    let client = Client::new();
    let mut request_json = serde_json::json!({
        "prompt": payload.request.prompt,
        "negative_prompt": payload.request.negative_prompt,
        "seed": payload.request.seed,
    });

    if let Some(path) = input_path_buf {
        let image_bytes = fs::read(&path)?;
        let b64 = BASE64_STANDARD.encode(&image_bytes);
        request_json["source_image"] = serde_json::Value::String(b64);
    }

    let response = client
        .post("http://127.0.0.1:8081/send")
        .json(&request_json)
        .send()?;

    if !response.status().is_success() {
        return Err(HunyuanError::Unavailable(format!(
            "Hunyuan API error: {}",
            response.status()
        )));
    }

    let response_json: serde_json::Value = response.json()?;
    let uid = response_json["uid"]
        .as_str()
        .ok_or_else(|| HunyuanError::InvalidRequest("No UID in response".into()))?;

    // Poll for completion
    let mut status = String::from("pending");
    let mut asset_ids = Vec::new();
    let mut error_message: Option<String> = None;

    while status == "pending" || status == "processing" {
        // Check cancellation
        let cancelled = {
            let db = project.db.lock().unwrap();
            db.query_row(
                "SELECT cancellation_requested FROM jobs WHERE job_id=?1",
                [job_id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(true)
        };
        if cancelled {
            return Err(HunyuanError::InvalidRequest("Job cancelled".into()));
        }

        std::thread::sleep(std::time::Duration::from_secs(5));
        let status_response = client
            .get(&format!("http://127.0.0.1:8081/status/{}", uid))
            .send()?;
        let status_json: serde_json::Value = status_response.json()?;
        status = status_json["status"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        if status == "completed" {
            // Download the result
            let model_b64 = status_json["model_base64"]
                .as_str()
                .ok_or_else(|| HunyuanError::InvalidRequest("No model in response".into()))?;
            let model_bytes = BASE64_STANDARD.decode(model_b64)?;

            // Save the model
            let asset_id = Uuid::now_v7().to_string();
            let masters = project.root.join("assets").join("masters");
            fs::create_dir_all(&masters)?;
            let master_path = masters.join(format!("{}.glb", asset_id));
            let temp_path = masters.join(format!(".{}.glb.tmp", asset_id));
            fs::write(&temp_path, &model_bytes)?;
            fs::rename(&temp_path, &master_path)?;

            // Save to database
            let now = now_ms();
            let checksum = format!("{:x}", Sha256::digest(&model_bytes));
            let mut db = project.db.lock().unwrap();
            let tx = db.transaction()?;
            tx.execute(
                "INSERT INTO assets (asset_id, project_id, original_filename, managed_master_path, file_size, checksum, imported_at_ms, status, source_type, media_kind, media_container, media_format, validation_level, model_metadata_schema_version, model_metadata_json) VALUES (?1,?2,'generated-model.glb',?3,?4,?5,?6,'ready','generated','model3d','GLB','glTF 2.0',?6,1,?7)",
                rusqlite::params![
                    asset_id,
                    project.manifest.project_id,
                    master_path.to_string_lossy(),
                    model_bytes.len() as u64,
                    checksum,
                    now,
                    "structural",
                    "{}"
                ],
            )?;
            tx.execute(
                "INSERT INTO asset_provenance (asset_id, parent_asset_id, source_type, provider_id, provider_version, model_identifier, provider_license_state, provider_license_ref, model_license_state, model_license_ref, commercial_use_allowed, prompt, negative_prompt, actual_seed, generation_settings_version, generation_settings_json, generating_job_id, generated_at_ms) VALUES (?1,?2,'generated',?3,?4,?5,'unknown',NULL,'unknown',NULL,NULL,?6,?7,?8,1,?9,?10,?11)",
                rusqlite::params![
                    asset_id,
                    payload.request.source_asset_id,
                    "hunyuan3d",
                    "1.0",
                    "foundation.hunyuan.v1",
                    payload.request.prompt,
                    payload.request.negative_prompt,
                    payload.request.seed.unwrap_or(0),
                    serde_json::json!({
                        "schemaVersion": 1,
                        "mode": payload.request.mode,
                        "sourceAssetId": payload.request.source_asset_id,
                        "quality": payload.request.quality,
                        "outputFormat": "glb",
                        "validationLevel": "structural"
                    }).to_string(),
                    now
                ],
            )?;
            tx.execute(
                "UPDATE jobs SET status='completed',updated_at_ms=?1,completed_at_ms=?1,progress=100,owner_token=NULL WHERE job_id=?2 AND status='running' AND owner_token=?3 AND cancellation_requested=0",
                rusqlite::params![now, job_id, owner],
            )?;
            tx.execute(
                "INSERT INTO job_events (job_id,event_type,from_status,to_status,attempt_count,created_at_ms) SELECT job_id,'completed','running','completed',attempt_count,?1 FROM jobs WHERE job_id=?2",
                rusqlite::params![now, job_id],
            )?;
            tx.commit()?;

            asset_ids.push(asset_id);
        } else if status == "failed" {
            error_message = status_json["error"].as_str().map(|s| s.to_string());
            break;
        }
    }

    if status == "failed" {
        return Err(HunyuanError::Unavailable(
            error_message.unwrap_or_else(|| "Hunyuan generation failed".into()),
        ));
    }

    Ok(HunyuanGenerationResult {
        job_id: job_id.to_string(),
        status: "completed".to_string(),
        asset_ids,
        error_message: None,
    })
}

#[derive(Debug, Error)]
pub enum HunyuanError {
    #[error("invalid hunyuan request: {0}")]
    InvalidRequest(String),
    #[error("hunyuan generation unavailable: {0}")]
    Unavailable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),
    #[error(transparent)]
    Validation(#[from] crate::model3d_validation::ModelValidationError),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::tempdir;

    fn request(source_asset_id: Option<String>) -> HunyuanGenerationRequest {
        HunyuanGenerationRequest {
            schema_version: 1,
            mode: if source_asset_id.is_some() {
                HunyuanMode::ImageTo3d
            } else {
                HunyuanMode::TextTo3d
            },
            prompt: " model ".into(),
            negative_prompt: None,
            source_asset_id,
            profile: HUNYUAN_PROFILE_ID.into(),
            quality: "standard".into(),
            seed: None,
            output_format: "glb".into(),
        }
    }

    #[test]
    fn contracts_are_strict_and_capability_values_are_exact() {
        assert_eq!(request(None).normalize().unwrap().prompt, "model");
        let mut value = serde_json::to_value(request(None)).unwrap();
        value["endpoint"] = serde_json::json!("http://example");
        assert!(serde_json::from_value::<HunyuanGenerationRequest>(value).is_err());
        assert!(
            request(Some(Uuid::now_v7().to_string()))
                .normalize()
                .is_ok()
        );
        assert_eq!(
            serde_json::to_string(&crate::providers::Capability::HunyuanTextTo3d).unwrap(),
            "\"hunyuan.text_to_3d\""
        );
        assert_eq!(
            serde_json::to_string(&crate::providers::Capability::HunyuanImageTo3d).unwrap(),
            "\"hunyuan.image_to_3d\""
        );
    }

    #[test]
    fn image_source_is_managed_intact_decodable_and_read_only() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let id = Uuid::now_v7().to_string();
        let masters = project.root.join("assets").join("masters");
        fs::create_dir_all(&masters).unwrap();
        let path = masters.join(format!("{id}.png"));
        let mut cursor = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        let bytes = cursor.into_inner();
        fs::write(&path, &bytes).unwrap();
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        project.with_db(|db| db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'source.png',?3,?4,?5,1,'ready','image')", rusqlite::params![id, project.manifest.project_id, path.to_string_lossy(), bytes.len(), checksum]).map(|_| ())).unwrap();
        let source = resolve_source(&project, &request(Some(id.clone())).normalize().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(source._bytes, bytes);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, b"tampered").unwrap();
        assert!(resolve_source(&project, &request(Some(id)).normalize().unwrap()).is_err());
        assert!(
            resolve_source(
                &project,
                &request(Some(Uuid::now_v7().to_string()))
                    .normalize()
                    .unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn oversized_source_dimensions_are_rejected_before_full_decode() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let id = Uuid::now_v7().to_string();
        let masters = project.root.join("assets").join("masters");
        fs::create_dir_all(&masters).unwrap();
        let path = masters.join(format!("{id}.jpg"));
        let bytes = vec![
            0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08, 0xff, 0xff, 0xff, 0xff, 0x03, 0x01, 0x11,
            0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00, 0xff, 0xd9,
        ];
        fs::write(&path, &bytes).unwrap();
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        project.with_db(|db| db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'huge.jpg',?3,?4,?5,1,'ready','image')", rusqlite::params![id, project.manifest.project_id, path.to_string_lossy(), bytes.len(), checksum]).map(|_| ())).unwrap();
        let error = resolve_source(&project, &request(Some(id)).normalize().unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("megapixel") || error.to_string().contains("dimensions")
        );
    }
}

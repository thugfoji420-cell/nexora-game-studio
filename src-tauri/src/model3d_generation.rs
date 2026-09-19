use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Read};
use thiserror::Error;
use uuid::Uuid;

#[cfg(test)]
use crate::model3d_validation::{ModelFormat, ValidatedModel};
use crate::{model3d_validation, project::ProjectState};

pub const MODEL3D_JOB_TYPE: &str = "model3d.generate";
pub const PROFILE_ID: &str = "foundation.glb.structural.v1";
const MAX_PROMPT_CHARS: usize = 2000;
const MAX_SOURCE_BYTES: u64 = 40 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 64 * 1024 * 1024;
#[cfg(test)]
const FIXTURE_MODEL_IDENTIFIER: &str = "test.fixture.triangle.v1";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Model3dMode {
    #[serde(rename = "text_to_3d")]
    TextTo3d,
    #[serde(rename = "image_to_3d")]
    ImageTo3d,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model3dGenerationRequest {
    pub schema_version: u32,
    pub mode: Model3dMode,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub source_asset_id: Option<String>,
    pub profile: String,
    pub quality: String,
    pub seed: Option<i64>,
    pub output_format: String,
}

impl Model3dGenerationRequest {
    pub fn normalize(mut self) -> Result<Self, Model3dError> {
        self.prompt = self.prompt.trim().to_owned();
        self.negative_prompt = self.negative_prompt.map(|value| value.trim().to_owned());
        let source_valid = match self.mode {
            Model3dMode::TextTo3d => self.source_asset_id.is_none(),
            Model3dMode::ImageTo3d => self.source_asset_id.as_deref().is_some_and(valid_uuid),
        };
        if self.schema_version != 1
            || !source_valid
            || self.prompt.is_empty()
            || self.prompt.chars().count() > MAX_PROMPT_CHARS
            || self
                .negative_prompt
                .as_ref()
                .is_some_and(|value| value.chars().count() > MAX_PROMPT_CHARS)
            || self.profile != PROFILE_ID
            || self.quality != "standard"
            || self.seed.is_some_and(|seed| seed < 0)
            || self.output_format != "glb"
        {
            return Err(Model3dError::InvalidRequest(
                "request is outside model3d generation v1 bounds".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model3dJobPayload {
    pub provider_id: String,
    pub provider_version: String,
    pub request: Model3dGenerationRequest,
}

pub fn validate_payload(payload: &Model3dJobPayload) -> Result<(), Model3dError> {
    if payload.provider_id.is_empty()
        || payload.provider_id.len() > 128
        || payload.provider_version.is_empty()
        || payload.provider_version.len() > 64
    {
        return Err(Model3dError::InvalidRequest(
            "invalid model3d provider identity".into(),
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
    request: &Model3dGenerationRequest,
) -> Result<Option<SourceImage>, Model3dError> {
    let Some(id) = request.source_asset_id.as_deref() else {
        return Ok(None);
    };
    let (path, registered_size, registered_checksum): (String, u64, String) = project.db.lock().unwrap().query_row(
        "SELECT managed_master_path,file_size,checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready' AND media_kind='image' AND (COALESCE(source_type, '') <> 'generated' OR EXISTS (SELECT 1 FROM asset_approvals WHERE asset_id=assets.asset_id AND status='approved'))",
        rusqlite::params![id, project.manifest.project_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).map_err(|_| Model3dError::InvalidRequest("sourceAssetId must reference a READY image in the current project".into()))?;
    let canonical = dunce::canonicalize(path)
        .map_err(|_| Model3dError::InvalidRequest("source image master is missing".into()))?;
    let masters = dunce::canonicalize(project.root.join("assets").join("masters"))?;
    let metadata = fs::metadata(&canonical)?;
    if !canonical.starts_with(&masters)
        || !metadata.is_file()
        || metadata.len() != registered_size
        || metadata.len() == 0
        || metadata.len() > MAX_SOURCE_BYTES
    {
        return Err(Model3dError::InvalidRequest(
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
        return Err(Model3dError::InvalidRequest(
            "source image size or checksum integrity failed".into(),
        ));
    }
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| Model3dError::InvalidRequest("source image format is not recognized".into()))?
        .into_dimensions()
        .map_err(|_| Model3dError::InvalidRequest("source image dimensions are invalid".into()))?;
    if width == 0
        || height == 0
        || u64::from(width)
            .checked_mul(u64::from(height))
            .is_none_or(|pixels| pixels > MAX_SOURCE_PIXELS)
    {
        return Err(Model3dError::InvalidRequest(
            "source image exceeds the 64 megapixel workload bound".into(),
        ));
    }
    image::load_from_memory(&bytes)
        .map_err(|_| Model3dError::InvalidRequest("source image is not fully decodable".into()))?;
    Ok(Some(SourceImage { _bytes: bytes }))
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct StagedModel {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub checksum: String,
    pub validated: ValidatedModel,
}

#[cfg(test)]
fn staging_root(project: &ProjectState, job_id: &str) -> Result<std::path::PathBuf, Model3dError> {
    if !valid_uuid(job_id) {
        return Err(Model3dError::InvalidRequest("invalid job identity".into()));
    }
    let root = project.root.join(".nexora").join("jobs");
    fs::create_dir_all(&root)?;
    let canonical_root = dunce::canonicalize(&root)?;
    let path = canonical_root.join(job_id).join("model3d");
    fs::create_dir_all(&path)?;
    let path = dunce::canonicalize(path)?;
    if !path.starts_with(canonical_root) {
        return Err(Model3dError::InvalidRequest(
            "unsafe model staging path".into(),
        ));
    }
    Ok(path)
}

#[cfg(test)]
pub fn stage_bytes(
    project: &ProjectState,
    job_id: &str,
    bytes: &[u8],
) -> Result<StagedModel, Model3dError> {
    let validated = model3d_validation::validate(bytes, ModelFormat::Glb)?;
    let root = staging_root(project, job_id)?;
    let path = root.join("output.glb");
    let temporary = root.join("output.glb.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &path)?;
    Ok(StagedModel {
        path,
        size: bytes.len() as u64,
        checksum: format!("{:x}", Sha256::digest(bytes)),
        validated,
    })
}

pub fn purge_staging(project: &ProjectState, job_id: &str) {
    if valid_uuid(job_id) {
        let _ = fs::remove_dir_all(
            project
                .root
                .join(".nexora")
                .join("jobs")
                .join(job_id)
                .join("model3d"),
        );
    }
}

#[cfg(test)]
pub fn promote(
    project: &ProjectState,
    job_id: &str,
    owner: &str,
    payload: &Model3dJobPayload,
    output: &StagedModel,
) -> Result<bool, Model3dError> {
    if !project.execution_enabled() || !valid_uuid(job_id) {
        return Ok(false);
    }
    let existing: Option<String> = project
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    if existing.is_some() {
        return Ok(true);
    }
    let asset_id = job_id.to_owned();
    let masters = project.root.join("assets").join("masters");
    fs::create_dir_all(&masters)?;
    let masters = dunce::canonicalize(masters)?;
    let master = masters.join(format!("{asset_id}.glb"));
    let temporary = masters.join(format!(".{asset_id}.glb.tmp"));
    if let Err(error) = fs::copy(&output.path, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temporary, &master) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    let result = (|| -> Result<bool, Model3dError> {
        let mut db = project.db.lock().unwrap();
        let tx = db.transaction().map_err(database)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1",
                [job_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(database)?;
        if existing.is_some() {
            return Ok(true);
        }
        let (status, cancelled): (String, bool) = tx.query_row("SELECT status,cancellation_requested FROM jobs WHERE job_id=?1 AND owner_token=?2 AND job_type='model3d.generate'", rusqlite::params![job_id, owner], |row| Ok((row.get(0)?, row.get(1)?))).map_err(database)?;
        if status != "running" || cancelled || !project.execution_enabled() {
            return Ok(false);
        }
        let now = now_ms();
        let metadata_json = serde_json::to_string(&output.validated.metadata)?;
        let settings = serde_json::to_string(&serde_json::json!({
            "schemaVersion": 1,
            "mode": payload.request.mode,
            "sourceAssetId": payload.request.source_asset_id,
            "profile": payload.request.profile,
            "quality": payload.request.quality,
            "outputFormat": payload.request.output_format,
            "validationLevel": output.validated.validation_level
        }))?;
        if settings.len() > 16384 {
            return Err(Model3dError::InvalidRequest(
                "generation settings exceed storage bound".into(),
            ));
        }
        tx.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind,media_container,media_format,validation_level,model_metadata_schema_version,model_metadata_json) VALUES (?1,?2,'generated-model.glb',?3,?4,?5,?6,'ready','generated','model3d','GLB','glTF 2.0',?7,1,?8)", rusqlite::params![asset_id, project.manifest.project_id, master.to_string_lossy(), output.size, output.checksum, now, output.validated.validation_level, metadata_json]).map_err(database)?;
        tx.execute("INSERT INTO asset_provenance (asset_id,parent_asset_id,source_type,provider_id,provider_version,model_identifier,provider_license_state,provider_license_ref,model_license_state,model_license_ref,commercial_use_allowed,prompt,negative_prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) VALUES (?1,?2,'generated',?3,?4,?5,'unknown',NULL,'unknown',NULL,NULL,?6,?7,?8,1,?9,?10,?11)", rusqlite::params![asset_id, payload.request.source_asset_id, payload.provider_id, payload.provider_version, FIXTURE_MODEL_IDENTIFIER, payload.request.prompt, payload.request.negative_prompt, payload.request.seed.ok_or_else(|| Model3dError::InvalidRequest("fixture seed was not normalized".into()))?, settings, job_id, now]).map_err(database)?;
        let changed = tx.execute("UPDATE jobs SET status='completed',updated_at_ms=?1,completed_at_ms=?1,progress=100,owner_token=NULL WHERE job_id=?2 AND status='running' AND owner_token=?3 AND cancellation_requested=0", rusqlite::params![now, job_id, owner]).map_err(database)?;
        if changed != 1 {
            return Ok(false);
        }
        tx.execute("INSERT INTO job_events (job_id,event_type,from_status,to_status,attempt_count,created_at_ms) SELECT job_id,'completed','running','completed',attempt_count,?1 FROM jobs WHERE job_id=?2", rusqlite::params![now, job_id]).map_err(database)?;
        tx.commit().map_err(database)?;
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
pub struct Model3dGenerationResult {
    pub job_id: String,
    pub status: String,
    pub asset_ids: Vec<String>,
}

pub fn get_result(
    project: &ProjectState,
    job_id: &str,
) -> Result<Model3dGenerationResult, Model3dError> {
    if !valid_uuid(job_id) {
        return Err(Model3dError::InvalidRequest("invalid job id".into()));
    }
    let db = project.db.lock().unwrap();
    let status = db
        .query_row(
            "SELECT status FROM jobs WHERE job_id=?1 AND job_type='model3d.generate'",
            [job_id],
            |row| row.get(0),
        )
        .map_err(database)?;
    let mut statement = db
        .prepare(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1 ORDER BY asset_id",
        )
        .map_err(database)?;
    let asset_ids = statement
        .query_map([job_id], |row| row.get(0))
        .map_err(database)?
        .collect::<Result<Vec<String>, _>>()
        .map_err(database)?;
    Ok(Model3dGenerationResult {
        job_id: job_id.into(),
        status,
        asset_ids,
    })
}

#[cfg(test)]
pub(crate) fn fixture_output() -> Vec<u8> {
    crate::model3d_validation::tests::fixture_glb()
}

#[derive(Debug, Error)]
pub enum Model3dError {
    #[error("invalid model3d request: {0}")]
    InvalidRequest(String),
    #[error("model3d generation unavailable: {0}")]
    Unavailable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Validation(#[from] model3d_validation::ModelValidationError),
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value)
}
#[cfg(test)]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn database(error: rusqlite::Error) -> Model3dError {
    Model3dError::InvalidRequest(error.to_string())
}
#[cfg(test)]
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::tempdir;

    fn request(source_asset_id: Option<String>) -> Model3dGenerationRequest {
        Model3dGenerationRequest {
            schema_version: 1,
            mode: if source_asset_id.is_some() {
                Model3dMode::ImageTo3d
            } else {
                Model3dMode::TextTo3d
            },
            prompt: " model ".into(),
            negative_prompt: None,
            source_asset_id,
            profile: PROFILE_ID.into(),
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
        assert!(serde_json::from_value::<Model3dGenerationRequest>(value).is_err());
        assert!(
            request(Some(Uuid::now_v7().to_string()))
                .normalize()
                .is_ok()
        );
        assert_eq!(
            serde_json::to_string(&crate::providers::Capability::Model3dTextTo3d).unwrap(),
            "\"model3d.text_to_3d\""
        );
        assert_eq!(
            serde_json::to_string(&crate::providers::Capability::Model3dImageTo3d).unwrap(),
            "\"model3d.image_to_3d\""
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

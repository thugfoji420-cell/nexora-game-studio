use crate::{model3d_validation, project::ProjectState};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fs;
use thiserror::Error;
use uuid::Uuid;

pub const MODEL3D_PROCESSING_PROFILE_ID: &str = "foundation.processing.v1";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingQuality {
    #[serde(rename = "master")]
    Master,
    #[serde(rename = "mobile_high")]
    MobileHigh,
    #[serde(rename = "mobile_balanced")]
    MobileBalanced,
    #[serde(rename = "mobile_low")]
    MobileLow,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model3dProcessingResult {
    pub job_id: String,
    pub status: String,
    pub asset_ids: Vec<String>,
    pub processing_stage: Option<String>,
    pub progress: u32,
    pub output_master_path: Option<String>,
    pub lod0_path: Option<String>,
    pub lod1_path: Option<String>,
    pub lod2_path: Option<String>,
    pub vehicle_analysis_path: Option<String>,
    pub material_status: Option<String>,
    pub pre_analysis_report: Option<ProcessingReport>,
    pub post_analysis_report: Option<ProcessingReport>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingReport {
    pub vertices_before: u64,
    pub vertices_after: u64,
    pub faces_before: u64,
    pub faces_after: u64,
    pub triangles_before: u64,
    pub triangles_after: u64,
    pub objects_before: u32,
    pub objects_after: u32,
    pub materials_before: u32,
    pub materials_after: u32,
    pub duplicate_vertices_removed: u64,
    pub degenerate_faces_removed: u64,
    pub loose_geometry_removed: u64,
    pub normals_recalculated: bool,
    pub transforms_normalized: bool,
    pub scale_normalized: bool,
    pub orientation_normalized: bool,
    pub duplicate_geometry_removed: u64,
    pub tiny_components_removed: u64,
    pub lod0_triangles: Option<u64>,
    pub lod1_triangles: Option<u64>,
    pub lod2_triangles: Option<u64>,
    #[serde(default)]
    pub collision_generated: Option<bool>,
    #[serde(default)]
    pub collision_triangles: Option<u64>,
    #[serde(default)]
    pub quality_score: Option<u32>,
    #[serde(default)]
    pub quality_rating: Option<String>,
    #[serde(default)]
    pub category_detected: String,
    #[serde(default)]
    pub category_confidence: f32,
    #[serde(default)]
    pub category_needs_review: bool,
    #[serde(default)]
    pub category_details: serde_json::Value,
    pub vehicle_detected: bool,
    pub wheel_candidates: u32,
    pub wheel_separation_possible: bool,
    pub vehicle_confidence: f32,
    pub material_status: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

pub fn get_result(
    project: &ProjectState,
    job_id: &str,
) -> Result<Model3dProcessingResult, Model3dProcessingError> {
    if !valid_uuid(job_id) {
        return Err(Model3dProcessingError::InvalidRequest(
            "invalid job id".into(),
        ));
    }
    let db = project
        .db
        .lock()
        .map_err(|_| Model3dProcessingError::Internal("mutex poisoned".into()))?;
    let row: Option<(
        String,
        Option<String>,
        Option<u32>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = {
        db.query_row(
            "SELECT j.status, p.processing_stage, p.progress, p.output_master_path, p.lod0_path, p.lod1_path, p.lod2_path, p.vehicle_analysis_path, p.material_status, p.error_code, p.error_message, p.blender_log_path FROM model3d_processing_jobs p JOIN jobs j ON j.job_id = p.job_id WHERE p.job_id=?1",
            rusqlite::params![job_id],
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
            )),
        )
        .optional()
        .map_err(|e| Model3dProcessingError::Internal(e.to_string()))?
    };
    let (
        status,
        processing_stage,
        progress,
        output_master_path,
        lod0_path,
        lod1_path,
        lod2_path,
        vehicle_analysis_path,
        material_status,
        error_code,
        error_message,
        _blender_log_path,
    ) = row.ok_or_else(|| Model3dProcessingError::NotFound("job not found".into()))?;

    let mut statement = db
        .prepare(
            "SELECT asset_id FROM asset_provenance WHERE generating_job_id=?1 ORDER BY asset_id",
        )
        .map_err(|e| Model3dProcessingError::Internal(e.to_string()))?;
    let asset_ids = statement
        .query_map([job_id], |row| row.get(0))
        .map_err(|e| Model3dProcessingError::Internal(e.to_string()))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| Model3dProcessingError::Internal(e.to_string()))?;

    let pre_report = if let Some(path) = db
        .query_row(
            "SELECT pre_analysis_report_path FROM model3d_processing_jobs WHERE job_id=?1",
            rusqlite::params![job_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    {
        read_report(&path)
    } else {
        None
    };

    let post_report = if let Some(path) = db
        .query_row(
            "SELECT post_analysis_report_path FROM model3d_processing_jobs WHERE job_id=?1",
            rusqlite::params![job_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    {
        read_report(&path)
    } else {
        None
    };

    Ok(Model3dProcessingResult {
        job_id: job_id.into(),
        status,
        asset_ids,
        processing_stage,
        progress: progress.unwrap_or(0),
        output_master_path,
        lod0_path,
        lod1_path,
        lod2_path,
        vehicle_analysis_path,
        material_status,
        pre_analysis_report: pre_report,
        post_analysis_report: post_report,
        error_code,
        error_message,
    })
}

fn read_report(path: &str) -> Option<ProcessingReport> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| uuid.to_string() == value)
}

#[derive(Debug, Error)]
pub enum Model3dProcessingError {
    #[error("invalid model3d processing request: {0}")]
    InvalidRequest(String),
    #[error("model3d processing unavailable: {0}")]
    Unavailable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("validation error: {0}")]
    Validation(#[from] model3d_validation::ModelValidationError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_uuid() {
        assert!(valid_uuid("123e4567-e89b-12d3-a456-426614174000"));
        assert!(!valid_uuid("invalid-uuid"));
    }
}

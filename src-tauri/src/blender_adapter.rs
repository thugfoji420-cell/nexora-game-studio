use crate::{
    blender::BlenderInfo, model3d_processing, project::ProjectState,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlenderProcessingConfig {
    pub blender_executable: PathBuf,
    pub script_path: PathBuf,
    pub timeout_seconds: u64,
}

pub fn resolve_blender_executable(configured: Option<&Path>) -> Result<PathBuf, BlenderAdapterError> {
    let info = crate::blender::find_and_validate_blender(configured);
    if let Some(path_str) = info.executable_path {
        let p = PathBuf::from(path_str);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(BlenderAdapterError::NotFound(
        info.error.unwrap_or_else(|| "Blender executable could not be found".into()),
    ))
}

pub fn resolve_script_path(configured: Option<&Path>) -> Result<PathBuf, BlenderAdapterError> {
    if let Some(p) = configured {
        if p.is_absolute() && p.exists() {
            return Ok(p.to_path_buf());
        }
    }
    let candidates = [
        PathBuf::from("src-tauri/blender/nexora_mesh_processor.py"),
        PathBuf::from("blender/nexora_mesh_processor.py"),
        PathBuf::from("../src-tauri/blender/nexora_mesh_processor.py"),
        PathBuf::from("../blender/nexora_mesh_processor.py"),
        PathBuf::from("../../src-tauri/blender/nexora_mesh_processor.py"),
        PathBuf::from("../../blender/nexora_mesh_processor.py"),
    ];
    if let Ok(current) = std::env::current_dir() {
        for candidate in &candidates {
            let full = current.join(candidate);
            if full.exists() {
                return Ok(full);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for candidate in &candidates {
                let full = parent.join(candidate);
                if full.exists() {
                    return Ok(full);
                }
            }
        }
    }
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }
    Err(BlenderAdapterError::NotFound(
        "Blender script nexora_mesh_processor.py not found".into(),
    ))
}

impl Default for BlenderProcessingConfig {
    fn default() -> Self {
        let blender_executable = resolve_blender_executable(None).unwrap_or_else(|_| {
            PathBuf::from(r"C:\Program Files\Blender Foundation\Blender 5.2\blender.exe")
        });
        let script_path = resolve_script_path(None).unwrap_or_else(|_| {
            PathBuf::from("src-tauri/blender/nexora_mesh_processor.py")
        });
        Self {
            blender_executable,
            script_path,
            timeout_seconds: 3600,
        }
    }
}

pub struct BlenderAdapter {
    config: BlenderProcessingConfig,
}

impl BlenderAdapter {
    pub fn new(mut config: BlenderProcessingConfig) -> Result<Self, BlenderAdapterError> {
        if !config.blender_executable.exists() {
            config.blender_executable = resolve_blender_executable(None)?;
        }
        if !config.script_path.exists() {
            config.script_path = resolve_script_path(Some(&config.script_path))?;
        }
        Ok(Self { config })
    }

    pub fn validate(&self) -> Result<BlenderInfo, BlenderAdapterError> {
        let mut cmd = Command::new(&self.config.blender_executable);
        cmd.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let output = cmd.output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version = stdout
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .map(|s| s.to_string());

            Ok(BlenderInfo {
                installed: true,
                executable_path: Some(self.config.blender_executable.to_string_lossy().to_string()),
                version,
                valid: true,
                error: None,
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(BlenderInfo {
                installed: true,
                executable_path: Some(self.config.blender_executable.to_string_lossy().to_string()),
                version: None,
                valid: false,
                error: Some(format!("Blender exited with error: {}", stderr)),
            })
        }
    }

    pub fn process_model(
        &self,
        project: &ProjectState,
        job_id: &str,
        source_asset_path: &Path,
        profile: &str,
        quality: &str,
    ) -> Result<model3d_processing::Model3dProcessingResult, BlenderAdapterError> {
        let job_id = Uuid::parse_str(job_id)
            .map_err(|_| BlenderAdapterError::ValidationFailed("invalid job UUID".into()))?;

        let job_output_dir = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(job_id.to_string())
            .join("model3d_processing");
        fs::create_dir_all(&job_output_dir)?;

        let input_path = if source_asset_path.is_absolute() {
            source_asset_path.to_path_buf()
        } else {
            project.root.join(source_asset_path)
        };

        if !input_path.exists() {
            return Err(BlenderAdapterError::ValidationFailed(format!(
                "Source asset not found: {}",
                input_path.display()
            )));
        }

        let report_path = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(job_id.to_string())
            .join("model3d_processing")
            .join("processing_report.json");
        let log_path = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(job_id.to_string())
            .join("model3d_processing")
            .join("processing.log");

        let script_path = resolve_script_path(Some(&self.config.script_path))?;

        eprintln!("Starting Blender processing for job {}", job_id);
        eprintln!("Input: {}", input_path.display());
        eprintln!("Output dir: {}", job_output_dir.display());
        eprintln!("Profile: {}, Quality: {}", profile, quality);

        let mut cmd = Command::new(&self.config.blender_executable);
        cmd.arg("--background")
            .arg("--python")
            .arg(&script_path)
            .arg("--")
            .arg("--input")
            .arg(&input_path)
            .arg("--output")
            .arg(&job_output_dir)
            .arg("--profile")
            .arg(profile)
            .arg("--quality")
            .arg(quality)
            .arg("--report")
            .arg(&*report_path.to_string_lossy())
            .arg("--log")
            .arg(&*log_path.to_string_lossy())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let _timeout = std::time::Duration::from_secs(self.config.timeout_seconds);
        let child = cmd.spawn()?;
        let start = std::time::Instant::now();
        let output = child.wait_with_output()?;
        let elapsed = start.elapsed();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        eprintln!(
            "Blender processing completed in {:.2}s with status: {}",
            elapsed.as_secs_f32(),
            output.status
        );
        if !stderr.is_empty() {
            eprintln!("Blender stderr: {}", stderr);
        }
        eprintln!("Blender stdout: {}", stdout);

        if !output.status.success() {
            return Err(BlenderAdapterError::ExecutionFailed(format!(
                "Blender exited with code {:?}: {}",
                output.status.code(),
                stderr
            )));
        }

        // Read the processing report
        let report_path = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(job_id.to_string())
            .join("model3d_processing")
            .join("processing_report.json");
        let report_content = fs::read_to_string(&report_path)?;
        let report: serde_json::Value = serde_json::from_str(&report_content)?;

        // Build the result
        let status = report["final_status"].as_str().unwrap_or("UNKNOWN");
        let job_output_dir = project
            .root
            .join(".nexora")
            .join("jobs")
            .join(job_id.to_string())
            .join("model3d_processing");
        let asset_ids = self.collect_output_assets(&job_output_dir)?;

        Ok(model3d_processing::Model3dProcessingResult {
            job_id: job_id.to_string(),
            status: status.to_string(),
            asset_ids,
            processing_stage: report["stages"]
                .get("complete")
                .and_then(|v| v["status"].as_str())
                .map(|s| s.to_string()),
            progress: report["stages"]
                .get("optimize")
                .and_then(|v| v["progress"].as_u64())
                .map(|v| v as u32)
                .unwrap_or(100),
            output_master_path: self.find_output_file(&job_output_dir, "clean_master.glb"),
            lod0_path: self.find_output_file(&job_output_dir, "vehicle_lod0.glb"),
            lod1_path: self.find_output_file(&job_output_dir, "vehicle_lod1.glb"),
            lod2_path: self.find_output_file(&job_output_dir, "vehicle_lod2.glb"),
            vehicle_analysis_path: self.find_output_file(&job_output_dir, "vehicle_analysis.json"),
            material_status: report["material_status"].as_str().map(|s| s.to_string()),
            pre_analysis_report: report["stages"]
                .get("analyze")
                .and_then(|v| v.get("analysis"))
                .and_then(|v| {
                    serde_json::from_value::<model3d_processing::ProcessingReport>(v.clone()).ok()
                }),
            post_analysis_report: {
                let mut post_report = report["stages"]
                    .get("validate")
                    .and_then(|v| v.get("validation"))
                    .and_then(|v| {
                        serde_json::from_value::<model3d_processing::ProcessingReport>(v.clone()).ok()
                    });
                // Merge category analysis into post report
                if let Some(cat_analysis) = report["stages"]
                    .get("category_analysis")
                    .and_then(|v| v.get("analysis"))
                {
                    if let Some(ref mut pr) = post_report {
                        pr.category_detected = cat_analysis.get("profile")
                            .or(cat_analysis.get("body_object"))
                            .or(cat_analysis.get("main_object"))
                            .or(cat_analysis.get("rig_status"))
                            .map(|v| v.as_str().unwrap_or("").to_string())
                            .unwrap_or_default();
                        pr.category_confidence = cat_analysis.get("confidence")
                            .or(cat_analysis.get("humanoid_confidence"))
                            .or(cat_analysis.get("modular_score"))
                            .and_then(|v| v.as_f64())
                            .map(|v| v as f32)
                            .unwrap_or(0.0);
                        pr.category_needs_review = cat_analysis.get("needs_review")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        pr.category_details = cat_analysis.clone();
                    }
                }
                post_report
            },
            error_code: None,
            error_message: if report["errors"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true)
            {
                None
            } else {
                Some(
                    report["errors"][0]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                )
            },
        })
    }

    fn collect_output_assets(&self, output_dir: &Path) -> Result<Vec<String>, BlenderAdapterError> {
        let mut assets = Vec::new();
        for entry in fs::read_dir(output_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "glb").unwrap_or(false) {
                assets.push(path.to_string_lossy().to_string());
            }
        }
        Ok(assets)
    }

    fn find_output_file(&self, output_dir: &Path, filename: &str) -> Option<String> {
        let path = output_dir.join(filename);
        if path.exists() {
            Some(path.to_string_lossy().to_string())
        } else {
            None
        }
    }
}

#[derive(Debug, Error)]
pub enum BlenderAdapterError {
    #[error("Blender not found: {0}")]
    NotFound(String),
    #[error("Blender validation failed: {0}")]
    ValidationFailed(String),
    #[error("Blender execution failed: {0}")]
    ExecutionFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub fn run_model3d_processing(
    project: &ProjectState,
    job_id: &str,
    _owner: &str,
    payload: &crate::jobs::Model3dProcessingJobPayload,
) -> Result<model3d_processing::Model3dProcessingResult, BlenderAdapterError> {
    let config = BlenderProcessingConfig::default();

    let adapter = BlenderAdapter::new(config)?;

    let source_asset_path = {
        let db = project.db.lock().unwrap();
        db.query_row(
            "SELECT managed_master_path FROM assets WHERE asset_id=?1 AND project_id=?2",
            rusqlite::params![payload.request.source_asset_id, project.manifest.project_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| BlenderAdapterError::ValidationFailed("source asset not found".into()))?
    };

    adapter.process_model(
        project,
        job_id,
        &PathBuf::from(source_asset_path),
        &payload.request.profile,
        &payload.request.quality,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = BlenderProcessingConfig::default();
        assert!(
            config
                .blender_executable
                .to_string_lossy()
                .contains("Blender")
        );
    }
}

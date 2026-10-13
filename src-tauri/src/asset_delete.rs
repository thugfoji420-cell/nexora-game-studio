//! Permanent asset deletion.
//!
//! Removes an asset's database rows and every managed file that belongs to
//! it: the master file under `assets/masters/`, its preview under
//! `assets/previews/`, and the job directories under `.nexora/jobs/` that
//! only this asset referenced. Assets that were delivered to a Unity project
//! are refused so the Unity side never ends up pointing at a deleted asset.

use crate::project::ProjectState;
use rusqlite::TransactionBehavior;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const BLOCKED_DELIVERY_STATUSES: &[&str] = &["delivered", "ready_for_import", "importing"];

#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedRows {
    pub assets: u64,
    pub approvals: u64,
    pub provenance: u64,
    pub processing_jobs: u64,
    pub unity_deliveries: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAssetReport {
    pub asset_id: String,
    pub deleted_rows: DeletedRows,
    /// Managed files that were removed from disk.
    pub deleted_files: Vec<String>,
    /// Files intentionally left on disk, with the reason.
    pub skipped_files: Vec<String>,
}

struct AssetRow {
    project_id: String,
    managed_master_path: String,
}

fn valid_asset_id(asset_id: &str) -> bool {
    !asset_id.is_empty()
        && asset_id.len() <= 64
        && asset_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Refuse to delete assets that Unity already received: the deployed copy in
/// the Unity project would silently outlive its source of truth.
pub fn check_not_delivered(project: &ProjectState, asset_id: &str) -> Result<(), String> {
    let blocked: i64 = project
        .db
        .lock()
        .unwrap()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM unity_deliveries WHERE asset_id=?1 AND status IN ({})",
                BLOCKED_DELIVERY_STATUSES
                    .iter()
                    .map(|status| format!("'{status}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            rusqlite::params![asset_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if blocked > 0 {
        return Err(
            "asset was delivered to a Unity project and cannot be deleted from here; \
             remove it from the Unity project first"
                .into(),
        );
    }
    Ok(())
}

/// Delete one asset: rows first (atomic), then managed files.
pub fn delete_asset(project: &ProjectState, asset_id: &str) -> Result<DeleteAssetReport, String> {
    if !valid_asset_id(asset_id) {
        return Err("invalid asset id".into());
    }
    check_not_delivered(project, asset_id)?;

    // Collect everything that must be read before rows disappear.
    let (row, generation_job_ids, processing_job_ids) = {
        let db = project.db.lock().unwrap();
        let row = db
            .query_row(
                "SELECT project_id, managed_master_path FROM assets WHERE asset_id=?1",
                rusqlite::params![asset_id],
                |row| {
                    Ok(AssetRow {
                        project_id: row.get(0)?,
                        managed_master_path: row.get(1)?,
                    })
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => "asset not found".to_string(),
                other => other.to_string(),
            })?;
        let mut generation_job_ids = Vec::new();
        {
            let mut stmt = db
                .prepare(
                    "SELECT DISTINCT generating_job_id FROM asset_provenance \
                     WHERE asset_id=?1 AND generating_job_id IS NOT NULL",
                )
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![asset_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            for job_id in rows {
                generation_job_ids.push(job_id.map_err(|error| error.to_string())?);
            }
        }
        let mut processing_job_ids = Vec::new();
        {
            let mut stmt = db
                .prepare(
                    "SELECT job_id FROM model3d_processing_jobs WHERE source_asset_id=?1",
                )
                .map_err(|error| error.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![asset_id], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            for job_id in rows {
                processing_job_ids.push(job_id.map_err(|error| error.to_string())?);
            }
        }
        (row, generation_job_ids, processing_job_ids)
    };

    if row.project_id != project.manifest.project_id {
        return Err("asset belongs to a different project".into());
    }

    // Job directories are shared when another asset points at the same job.
    let generation_job_ids = generation_job_ids
        .into_iter()
        .filter(|job_id| {
            !generation_job_shared(project, job_id, asset_id).unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let processing_job_ids = processing_job_ids
        .into_iter()
        .filter(|job_id| {
            !processing_job_shared(project, job_id, asset_id).unwrap_or(true)
        })
        .collect::<Vec<_>>();

    // Rows are removed atomically; only after a successful commit touch files.
    let deleted_rows = {
        let mut db = project.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let unity_deliveries = tx
            .execute(
                "DELETE FROM unity_deliveries WHERE asset_id=?1",
                rusqlite::params![asset_id],
            )
            .map_err(|error| error.to_string())?;
        let approvals = tx
            .execute(
                "DELETE FROM asset_approvals WHERE asset_id=?1",
                rusqlite::params![asset_id],
            )
            .map_err(|error| error.to_string())?;
        let provenance = tx
            .execute(
                "DELETE FROM asset_provenance WHERE asset_id=?1",
                rusqlite::params![asset_id],
            )
            .map_err(|error| error.to_string())?;
        let processing_jobs = tx
            .execute(
                "DELETE FROM model3d_processing_jobs WHERE source_asset_id=?1",
                rusqlite::params![asset_id],
            )
            .map_err(|error| error.to_string())?;
        let assets = tx
            .execute(
                "DELETE FROM assets WHERE asset_id=?1 AND project_id=?2",
                rusqlite::params![asset_id, row.project_id],
            )
            .map_err(|error| error.to_string())?;
        if assets != 1 {
            return Err("asset not found".into());
        }
        tx.commit().map_err(|error| error.to_string())?;
        DeletedRows {
            assets: assets as u64,
            approvals: approvals as u64,
            provenance: provenance as u64,
            processing_jobs: processing_jobs as u64,
            unity_deliveries: unity_deliveries as u64,
        }
    };

    let mut deleted_files = Vec::new();
    let mut skipped_files = Vec::new();

    let masters_dir = managed_dir(project, &["assets", "masters"]);
    match managed_subpath(&masters_dir, Path::new(&row.managed_master_path)) {
        Some(master_path) => {
            remove_file_reported(&master_path, &mut deleted_files, &mut skipped_files);
        }
        None => skipped_files.push(format!(
            "master outside managed storage left untouched: {}",
            row.managed_master_path
        )),
    }

    let preview_path = managed_dir(project, &["assets", "previews"]).join(format!(
        "{asset_id}_preview.png"
    ));
    remove_file_reported(&preview_path, &mut deleted_files, &mut skipped_files);

    let jobs_root = managed_dir(project, &[".nexora", "jobs"]);
    for job_id in generation_job_ids.iter().chain(processing_job_ids.iter()) {
        let job_dir = jobs_root.join(job_id);
        if job_dir.is_dir() {
            match fs::remove_dir_all(&job_dir) {
                Ok(()) => deleted_files.push(format_dir(&job_dir)),
                Err(error) => skipped_files.push(format!("{} ({error})", format_dir(&job_dir))),
            }
        }
    }

    Ok(DeleteAssetReport {
        asset_id: asset_id.to_string(),
        deleted_rows,
        deleted_files,
        skipped_files,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedDelete {
    pub asset_id: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAssetsReport {
    pub requested: usize,
    pub deleted: Vec<DeleteAssetReport>,
    pub failed: Vec<FailedDelete>,
}

/// Bulk delete: every asset is attempted independently, so one blocked asset
/// (delivered to Unity, missing, bad id) never blocks the rest of the batch.
/// Duplicate ids are collapsed before work begins.
pub fn delete_assets(
    project: &ProjectState,
    asset_ids: Vec<String>,
) -> Result<DeleteAssetsReport, String> {
    let mut unique_ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for asset_id in asset_ids {
        if seen.insert(asset_id.clone()) {
            unique_ids.push(asset_id);
        }
    }
    let mut report = DeleteAssetsReport {
        requested: unique_ids.len(),
        deleted: Vec::new(),
        failed: Vec::new(),
    };
    for asset_id in unique_ids {
        match delete_asset(project, &asset_id) {
            Ok(deleted) => report.deleted.push(deleted),
            Err(error) => report.failed.push(FailedDelete { asset_id, error }),
        }
    }
    Ok(report)
}

fn generation_job_shared(
    project: &ProjectState,
    job_id: &str,
    asset_id: &str,
) -> Result<bool, String> {
    let db = project.db.lock().unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM asset_provenance WHERE generating_job_id=?1 AND asset_id<>?2",
            rusqlite::params![job_id, asset_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(count > 0)
}

fn processing_job_shared(
    project: &ProjectState,
    job_id: &str,
    asset_id: &str,
) -> Result<bool, String> {
    let db = project.db.lock().unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM model3d_processing_jobs WHERE job_id=?1 AND source_asset_id<>?2",
            rusqlite::params![job_id, asset_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    Ok(count > 0)
}

fn managed_dir(project: &ProjectState, segments: &[&str]) -> PathBuf {
    let mut path = project.root().clone();
    for segment in segments {
        path.push(segment);
    }
    path
}

/// Only delete files that verifiably live inside the managed directory.
fn managed_subpath(managed_dir: &Path, stored_path: &Path) -> Option<PathBuf> {
    let canonical = dunce::canonicalize(stored_path).ok()?;
    let canonical_dir = dunce::canonicalize(managed_dir).ok()?;
    if canonical.starts_with(&canonical_dir) && canonical.is_file() {
        Some(canonical)
    } else {
        None
    }
}

fn remove_file_reported(
    path: &Path,
    deleted_files: &mut Vec<String>,
    skipped_files: &mut Vec<String>,
) {
    if !path.exists() {
        return;
    }
    match fs::remove_file(path) {
        Ok(()) => deleted_files.push(format_file(path)),
        Err(error) => skipped_files.push(format!("{} ({error})", format_file(path))),
    }
}

fn format_file(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn format_dir(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("jobs/{name}"))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectState;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn unique_asset_id() -> String {
        Uuid::now_v7().to_string()
    }

    fn insert_asset(project: &ProjectState, asset_id: &str) -> PathBuf {
        let masters = project.root().join("assets").join("masters");
        fs::create_dir_all(&masters).unwrap();
        let master = masters.join(format!("{asset_id}.glb"));
        fs::write(&master, b"glTF-binary-placeholder").unwrap();
        let generation_job_id = format!("gen-{asset_id}");
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO jobs (job_id,job_type,status,payload_json,payload_version,created_at_ms,updated_at_ms,progress,attempt_count,max_attempts,cancellation_requested,retryable) \
                     VALUES (?1,'hunyuan.generate','completed','{}',1,1,1,100,1,1,0,0)",
                    rusqlite::params![generation_job_id],
                )?;
                db.execute(
                    "INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind) \
                     VALUES (?1,?2,'test.glb',?3,25,'checksum',1,'ready','generated','model3d')",
                    rusqlite::params![asset_id, project.manifest.project_id, master.to_string_lossy()],
                )?;
                db.execute(
                    "INSERT INTO asset_provenance (asset_id,source_type,provider_id,provider_version,prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) \
                     VALUES (?1,'generated','local.hunyuan','1.0.0','test prompt',1,1,'{}',?2,1)",
                    rusqlite::params![asset_id, generation_job_id],
                )?;
                db.execute(
                    "INSERT INTO asset_approvals (asset_id,status,created_at_ms,updated_at_ms) VALUES (?1,'approved',1,1)",
                    rusqlite::params![asset_id],
                )?;
                Ok(())
            })
            .unwrap();
        let generation_dir = project.root().join(".nexora").join("jobs").join(format!("gen-{asset_id}"));
        fs::create_dir_all(&generation_dir).unwrap();
        fs::write(generation_dir.join("output.glb"), b"x").unwrap();
        master
    }

    #[test]
    fn delete_removes_rows_files_and_unshared_job_dir() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let asset_id = unique_asset_id();
        let master = insert_asset(&project, &asset_id);

        let report = delete_asset(&project, &asset_id).unwrap();
        assert_eq!(report.deleted_rows.assets, 1);
        assert_eq!(report.deleted_rows.approvals, 1);
        assert_eq!(report.deleted_rows.provenance, 1);
        assert_eq!(report.deleted_rows.unity_deliveries, 0);
        assert!(!master.exists(), "master file must be removed");
        assert!(!report.deleted_files.is_empty());
        assert!(
            !project
                .root()
                .join(".nexora")
                .join("jobs")
                .join(format!("gen-{asset_id}"))
                .exists(),
            "unshared generation job dir must be removed"
        );

        let remaining: i64 = project
            .with_db(|db| {
                db.query_row("SELECT COUNT(*) FROM assets WHERE asset_id=?1", [&asset_id], |row| {
                    row.get(0)
                })
            })
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn delete_is_refused_when_unity_delivery_exists() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let asset_id = unique_asset_id();
        insert_asset(&project, &asset_id);
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO jobs (job_id,job_type,status,payload_json,payload_version,created_at_ms,updated_at_ms,progress,attempt_count,max_attempts,cancellation_requested,retryable) \
                     VALUES ('pj-1','model3d.processing','completed','{}',1,1,1,100,1,1,0,0)",
                    [],
                )?;
                db.execute(
                    "INSERT INTO model3d_processing_jobs (job_id,source_asset_id,profile,quality,created_at_ms,updated_at_ms,progress) \
                     VALUES ('pj-1',?1,'vehicle','master',1,1,100)",
                    rusqlite::params![asset_id],
                )?;
                db.execute(
                    "INSERT INTO unity_deliveries (delivery_id,asset_id,approval_id,processing_job_id,approved_artifact_checksum,unity_project_root,unity_destination_folder,delivery_revision,status,created_at_ms,updated_at_ms) \
                     VALUES ('d-1',?1,'a','pj-1','c','C:/Unity','Assets/Nex',1,'delivered',1,1)",
                    rusqlite::params![asset_id],
                )?;
                Ok(())
            })
            .unwrap();

        let error = delete_asset(&project, &asset_id).unwrap_err();
        assert!(error.contains("delivered to a Unity project"), "{error}");

        // The asset row survives a refused delete.
        let remaining: i64 = project
            .with_db(|db| {
                db.query_row("SELECT COUNT(*) FROM assets WHERE asset_id=?1", [&asset_id], |row| {
                    row.get(0)
                })
            })
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn failed_delivery_does_not_block_deletion_and_job_dir_is_kept_when_shared() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let asset_id = unique_asset_id();
        let other_id = unique_asset_id();
        let master = insert_asset(&project, &asset_id);
        // A second asset produced by the SAME generation job.
        let generation_job_id = format!("gen-{asset_id}");
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO jobs (job_id,job_type,status,payload_json,payload_version,created_at_ms,updated_at_ms,progress,attempt_count,max_attempts,cancellation_requested,retryable) \
                     VALUES ('pj-x','model3d.processing','failed','{}',1,1,1,0,1,1,0,0)",
                    [],
                )?;
                db.execute(
                    "INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind) \
                     VALUES (?1,?2,'other.glb',?3,25,'checksum2',1,'ready','generated','model3d')",
                    rusqlite::params![other_id, project.manifest.project_id, master.to_string_lossy()],
                )?;
                db.execute(
                    "INSERT INTO asset_provenance (asset_id,source_type,provider_id,provider_version,prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) \
                     VALUES (?1,'generated','local.hunyuan','1.0.0','test prompt',1,1,'{}',?2,1)",
                    rusqlite::params![other_id, generation_job_id],
                )?;
                db.execute(
                    "INSERT INTO unity_deliveries (delivery_id,asset_id,approval_id,processing_job_id,approved_artifact_checksum,unity_project_root,unity_destination_folder,delivery_revision,status,created_at_ms,updated_at_ms) \
                     VALUES ('d-2',?1,'a','pj-x','c','C:/Unity','Assets/Nex',1,'failed',1,1)",
                    rusqlite::params![asset_id],
                )?;
                Ok(())
            })
            .unwrap();

        let report = delete_asset(&project, &asset_id).unwrap();
        assert_eq!(report.deleted_rows.unity_deliveries, 1);
        assert!(
            project
                .root()
                .join(".nexora")
                .join("jobs")
                .join(&generation_job_id)
                .exists(),
            "shared job dir must be kept for the sibling asset"
        );
        assert!(!master.exists());
    }

    #[test]
    fn bulk_delete_reports_each_asset_independently_and_deduplicates() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let first = unique_asset_id();
        let second = unique_asset_id();
        let blocked = unique_asset_id();
        insert_asset(&project, &first);
        insert_asset(&project, &second);
        insert_asset(&project, &blocked);
        // The blocked asset carries a delivered Unity record.
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO jobs (job_id,job_type,status,payload_json,payload_version,created_at_ms,updated_at_ms,progress,attempt_count,max_attempts,cancellation_requested,retryable) \
                     VALUES ('pj-b','model3d.processing','completed','{}',1,1,1,100,1,1,0,0)",
                    [],
                )?;
                db.execute(
                    "INSERT INTO model3d_processing_jobs (job_id,source_asset_id,profile,quality,created_at_ms,updated_at_ms,progress) \
                     VALUES ('pj-b',?1,'vehicle','master',1,1,100)",
                    rusqlite::params![blocked],
                )?;
                db.execute(
                    "INSERT INTO unity_deliveries (delivery_id,asset_id,approval_id,processing_job_id,approved_artifact_checksum,unity_project_root,unity_destination_folder,delivery_revision,status,created_at_ms,updated_at_ms) \
                     VALUES ('d-b',?1,'a','pj-b','c','C:/Unity','Assets/Nex',1,'delivered',1,1)",
                    rusqlite::params![blocked],
                )?;
                Ok(())
            })
            .unwrap();

        let report = delete_assets(
            &project,
            vec![
                first.clone(),
                second.clone(),
                blocked.clone(),
                first.clone(), // duplicate must collapse to one attempt
                "not-a-uuid".into(),
            ],
        )
        .unwrap();
        assert_eq!(report.requested, 4, "duplicates collapse");
        assert_eq!(report.deleted.len(), 2);
        assert_eq!(report.failed.len(), 2);
        assert_eq!(report.failed[0].asset_id, blocked);
        assert!(report.failed[0].error.contains("delivered to a Unity project"));
        assert_eq!(report.failed[1].asset_id, "not-a-uuid");

        // Both deletable assets really left the database.
        let remaining: i64 = project
            .with_db(|db| {
                db.query_row(
                    "SELECT COUNT(*) FROM assets WHERE asset_id IN (?1, ?2)",
                    rusqlite::params![first, second],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(remaining, 0);
        // The blocked asset survives.
        let blocked_left: i64 = project
            .with_db(|db| {
                db.query_row(
                    "SELECT COUNT(*) FROM assets WHERE asset_id=?1",
                    [&blocked],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(blocked_left, 1);
    }

    #[test]
    fn missing_asset_and_bad_ids_are_rejected() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        assert_eq!(delete_asset(&project, "does-not-exist").unwrap_err(), "asset not found");
        assert_eq!(delete_asset(&project, "../escape").unwrap_err(), "invalid asset id");
    }

    #[test]
    fn master_outside_managed_storage_is_never_deleted() {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Models").unwrap();
        let asset_id = unique_asset_id();
        let outside = dir.path().join("outside.glb");
        fs::write(&outside, b"x").unwrap();
        project
            .with_db(|db| {
                db.execute(
                    "INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) \
                     VALUES (?1,?2,'test.glb',?3,1,'checksum',1,'ready','model3d')",
                    rusqlite::params![asset_id, project.manifest.project_id, outside.to_string_lossy()],
                )?;
                Ok(())
            })
            .unwrap();

        let report = delete_asset(&project, &asset_id).unwrap();
        assert!(outside.exists(), "file outside managed storage must survive");
        assert!(report
            .skipped_files
            .iter()
            .any(|note| note.contains("outside managed storage")));
    }
}

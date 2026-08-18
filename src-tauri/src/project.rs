use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_FILENAME: &str = "nexora.project.json";
pub const DATABASE_FILENAME: &str = "nexora.db";
const LOCK_FILENAME: &str = ".nexora.lock";

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("invalid project root: {0}")]
    InvalidRoot(String),
    #[error("path traversal detected: {0}")]
    PathTraversal(String),
    #[error("project already exists at: {0}")]
    AlreadyExists(PathBuf),
    #[error("project not found at: {0}")]
    NotFound(PathBuf),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("lock error: {0}")]
    Lock(String),
    #[error("overlapping project: {0}")]
    Overlap(String),
    #[error("unsupported schema version: {0}")]
    SchemaVersion(u32),
    #[error("stale lock detected: {0}")]
    StaleLock(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub schema_version: u32,
    pub project_id: String,
    pub name: String,
    pub created_at_ms: u128,
    pub format_version: String,
}

impl ProjectManifest {
    pub fn new(name: String) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            project_id: Uuid::now_v7().to_string(),
            name,
            created_at_ms: now,
            format_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.schema_version != PROJECT_SCHEMA_VERSION {
            return Err(ProjectError::SchemaVersion(self.schema_version));
        }
        if self.project_id.is_empty() || self.name.is_empty() {
            return Err(ProjectError::Manifest("missing required fields".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub database_path: PathBuf,
    pub is_open: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LockData {
    pid: u32,
    acquired_at_ms: u128,
    host: String,
}

#[derive(Debug)]
pub struct ProjectState {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub db: Mutex<rusqlite::Connection>,
    lock_path: PathBuf,
    _lock_file: Mutex<Option<fs::File>>,
    execution_enabled: AtomicBool,
}

impl ProjectState {
    pub fn open(root: &Path) -> Result<Self, ProjectError> {
        let canonical_root = canonicalize_path(root)?;
        let manifest_path = canonical_root.join(MANIFEST_FILENAME);
        let database_path = canonical_root.join(DATABASE_FILENAME);
        let lock_path = canonical_root.join(LOCK_FILENAME);

        let manifest = load_manifest(&manifest_path)?;
        manifest.validate()?;

        let db = rusqlite::Connection::open(&database_path)?;
        Self::run_migrations(&db)?;

        let state = Self {
            root: canonical_root,
            manifest,
            db: Mutex::new(db),
            lock_path,
            _lock_file: Mutex::new(None),
            execution_enabled: AtomicBool::new(true),
        };

        state.acquire_lock()?;
        crate::model3d_import::recover(&state)
            .map_err(|error| ProjectError::Manifest(format!("model import recovery: {error}")))?;
        crate::jobs::recover(&state).map_err(|error| ProjectError::Manifest(error.to_string()))?;
        Ok(state)
    }

    pub fn create(root: &Path, name: &str) -> Result<Self, ProjectError> {
        let canonical_root = canonicalize_path(root)?;

        let manifest_path = canonical_root.join(MANIFEST_FILENAME);
        let database_path = canonical_root.join(DATABASE_FILENAME);
        let lock_path = canonical_root.join(LOCK_FILENAME);

        if manifest_path.exists() {
            return Err(ProjectError::AlreadyExists(canonical_root.clone()));
        }

        check_no_overlap(&canonical_root)?;

        fs::create_dir_all(&canonical_root)?;

        let manifest = ProjectManifest::new(name.to_string());
        save_manifest(&manifest_path, &manifest)?;

        let db = rusqlite::Connection::open(&database_path)?;
        Self::run_migrations(&db)?;

        let state = Self {
            root: canonical_root,
            manifest,
            db: Mutex::new(db),
            lock_path,
            _lock_file: Mutex::new(None),
            execution_enabled: AtomicBool::new(true),
        };

        state.acquire_lock()?;
        crate::model3d_import::recover(&state)
            .map_err(|error| ProjectError::Manifest(format!("model import recovery: {error}")))?;
        crate::jobs::recover(&state).map_err(|error| ProjectError::Manifest(error.to_string()))?;
        Ok(state)
    }

    fn run_migrations(db: &rusqlite::Connection) -> Result<(), ProjectError> {
        db.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        db.execute_batch(include_str!("../migrations/002_assets.sql"))?;
        db.execute_batch(include_str!("../migrations/003_jobs.sql"))?;
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 4 {
            let migration = db.execute_batch(include_str!(
                "../migrations/004_provider_diagnostic_jobs.sql"
            ));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 5 {
            let migration =
                db.execute_batch(include_str!("../migrations/005_image_generation.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 6 {
            let migration =
                db.execute_batch(include_str!("../migrations/006_video_generation.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 7 {
            let migration =
                db.execute_batch(include_str!("../migrations/007_model3d_foundation.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 8 {
            let migration = db.execute_batch(include_str!("../migrations/008_3d_processing.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }
        db.pragma_update(None, "foreign_keys", true)?;
        let foreign_key_error = db
            .prepare("PRAGMA foreign_key_check")?
            .query_map([], |_| Ok(()))?
            .next()
            .transpose()?;
        if foreign_key_error.is_some() {
            return Err(ProjectError::Manifest(
                "database foreign key check failed after migration".into(),
            ));
        }

        // Migration 9: unity_delivery
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 9 {
            let migration = db.execute_batch(include_str!("../migrations/009_unity_delivery.sql"));
            migration?;
        }

        // Migration 10: unity_project_targets
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 10 {
            let migration = db.execute_batch(include_str!("../migrations/010_unity_targets.sql"));
            migration?;
        }

        // Migration 11: add target_id to unity_deliveries
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 11 {
            db.execute(
                "ALTER TABLE unity_deliveries ADD COLUMN target_id TEXT REFERENCES unity_project_targets(target_id)",
                [],
            )?;
            db.execute(
                "CREATE INDEX IF NOT EXISTS idx_unity_deliveries_target ON unity_deliveries(target_id)",
                [],
            )?;
            db.execute(
                "INSERT INTO schema_version (version, applied_at_ms) VALUES (11, strftime('%s', 'now') * 1000)",
                [],
            )?;
        }

        Ok(())
    }

    fn acquire_lock(&self) -> Result<(), ProjectError> {
        let mut lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.lock_path)?;
        let lock_data = LockData {
            pid: std::process::id(),
            acquired_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            host: whoami::username(),
        };

        let metadata = fs::metadata(&self.lock_path)?;
        if metadata.len() > 0 {
            let existing: LockData = serde_json::from_slice(&fs::read(&self.lock_path)?)
                .map_err(|_| ProjectError::Lock("corrupt lock file".into()))?;
            return Err(ProjectError::StaleLock(format!(
                "project locked by pid {} on {} since {}",
                existing.pid, existing.host, existing.acquired_at_ms
            )));
        }

        serde_json::to_writer(&mut lock_file, &lock_data)?;
        lock_file.flush()?;

        *self._lock_file.lock().unwrap() = Some(lock_file);
        Ok(())
    }

    pub fn release_lock(&self) -> Result<(), ProjectError> {
        let mut guard = self._lock_file.lock().unwrap();
        if let Some(mut file) = guard.take() {
            let _ = file.set_len(0);
            let _ = file.flush();
        }
        Ok(())
    }

    pub fn execution_enabled(&self) -> bool {
        self.execution_enabled.load(Ordering::Acquire)
    }

    pub fn disable_execution(&self) {
        let _db = self.db.lock().unwrap();
        self.execution_enabled.store(false, Ordering::Release);
    }

    pub fn info(&self) -> ProjectInfo {
        ProjectInfo {
            root: self.root.clone(),
            manifest: self.manifest.clone(),
            database_path: self.root.join(DATABASE_FILENAME),
            is_open: true,
        }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn close(self) -> Result<(), ProjectError> {
        self.disable_execution();
        self.release_lock()
    }

    pub fn with_db<F, R>(&self, f: F) -> Result<R, ProjectError>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R, rusqlite::Error>,
    {
        let mut db = self.db.lock().unwrap();
        f(&mut db).map_err(ProjectError::Database)
    }
}

pub fn canonicalize_path(path: &Path) -> Result<PathBuf, ProjectError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let canonical = if absolute.exists() {
        dunce::canonicalize(&absolute).map_err(|e| {
            ProjectError::InvalidRoot(format!("cannot canonicalize {}: {}", absolute.display(), e))
        })?
    } else {
        let parent = absolute
            .parent()
            .ok_or_else(|| ProjectError::InvalidRoot("path has no parent".into()))?;
        let canonical_parent = dunce::canonicalize(parent).map_err(|e| {
            ProjectError::InvalidRoot(format!(
                "cannot canonicalize parent {}: {}",
                parent.display(),
                e
            ))
        })?;
        canonical_parent.join(absolute.file_name().unwrap())
    };

    let canonical_str = canonical.to_string_lossy();
    if canonical_str.contains("..") {
        return Err(ProjectError::PathTraversal(
            "canonical path contains parent references".into(),
        ));
    }

    Ok(canonical)
}

pub fn load_manifest(path: &Path) -> Result<ProjectManifest, ProjectError> {
    let content = fs::read_to_string(path).map_err(|e| ProjectError::Manifest(e.to_string()))?;
    let manifest: ProjectManifest =
        serde_json::from_str(&content).map_err(|e| ProjectError::Manifest(e.to_string()))?;
    Ok(manifest)
}

pub fn save_manifest(path: &Path, manifest: &ProjectManifest) -> Result<(), ProjectError> {
    let tmp = path.with_extension("json.tmp");
    let content = serde_json::to_vec_pretty(manifest)?;
    fs::write(&tmp, content)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn check_no_overlap(root: &Path) -> Result<(), ProjectError> {
    let canonical_root = canonicalize_path(root)?;

    let mut current = Some(canonical_root.as_path());
    while let Some(path) = current {
        let manifest_path = path.join(MANIFEST_FILENAME);
        if manifest_path.exists() {
            if let Ok(other) = load_manifest(&manifest_path) {
                return Err(ProjectError::Overlap(format!(
                    "project '{}' at {} overlaps with requested root {}",
                    other.name,
                    path.display(),
                    canonical_root.display()
                )));
            }
        }
        current = path.parent();
    }

    if canonical_root.exists() {
        for entry in fs::read_dir(&canonical_root)? {
            let entry = entry?;
            if entry.path().is_dir() {
                let manifest_path = entry.path().join(MANIFEST_FILENAME);
                if manifest_path.exists() {
                    if let Ok(other) = load_manifest(&manifest_path) {
                        return Err(ProjectError::Overlap(format!(
                            "project '{}' at {} overlaps with requested root {}",
                            other.name,
                            entry.path().display(),
                            canonical_root.display()
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

impl Drop for ProjectState {
    fn drop(&mut self) {
        let _ = self.release_lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn canonicalize_rejects_traversal() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let traversal = sub.join("..").join("sub");
        assert!(canonicalize_path(&traversal).is_ok());
    }

    #[test]
    fn create_and_open_project() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("TestProject");

        let state = ProjectState::create(&root, "Test Project").unwrap();
        assert_eq!(state.manifest.name, "Test Project");
        assert!(!state.manifest.project_id.is_empty());
        assert_eq!(state.manifest.schema_version, PROJECT_SCHEMA_VERSION);
        let project_id = state.manifest.project_id.clone();

        drop(state);

        let reopened = ProjectState::open(&root).unwrap();
        assert_eq!(reopened.manifest.project_id, project_id);
        reopened.close().unwrap();
    }

    #[test]
    fn reject_duplicate_project() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("TestProject");
        ProjectState::create(&root, "Test Project")
            .unwrap()
            .close()
            .unwrap();
        let err = ProjectState::create(&root, "Another").unwrap_err();
        assert!(matches!(err, ProjectError::AlreadyExists(_)));
    }

    #[test]
    fn reject_nested_project() {
        let dir = tempdir().unwrap();
        let parent = dir.path().join("Parent");
        let child = parent.join("Child");
        ProjectState::create(&parent, "Parent")
            .unwrap()
            .close()
            .unwrap();
        let err = ProjectState::create(&child, "Child").unwrap_err();
        assert!(matches!(err, ProjectError::Overlap(_)));
    }

    #[test]
    fn reject_parent_of_existing() {
        let dir = tempdir().unwrap();
        let child = dir.path().join("Child");
        let parent = child.parent().unwrap().to_path_buf();
        ProjectState::create(&child, "Child")
            .unwrap()
            .close()
            .unwrap();
        let err = ProjectState::create(&parent, "Parent").unwrap_err();
        assert!(matches!(err, ProjectError::Overlap(_)));
    }

    #[test]
    fn lock_prevents_second_writer() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("LockedProject");
        let first = ProjectState::create(&root, "Locked").unwrap();
        let err = ProjectState::open(&root).unwrap_err();
        assert!(matches!(err, ProjectError::StaleLock(_)));
        first.close().unwrap();

        let second = ProjectState::open(&root).unwrap();
        second.close().unwrap();
    }

    #[test]
    fn manifest_version_validation() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("VersionTest");
        fs::create_dir_all(&root).unwrap();
        let manifest = ProjectManifest {
            schema_version: 999,
            project_id: Uuid::now_v7().to_string(),
            name: "Bad".into(),
            created_at_ms: 0,
            format_version: "0.0.0".into(),
        };
        save_manifest(&root.join(MANIFEST_FILENAME), &manifest).unwrap();
        let err = ProjectState::open(&root).unwrap_err();
        assert!(matches!(err, ProjectError::SchemaVersion(999)));
    }

    #[test]
    fn jobs_migration_preserves_existing_assets() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("MigratedProject");
        fs::create_dir_all(&root).unwrap();
        let manifest = ProjectManifest::new("Migrated".into());
        save_manifest(&root.join(MANIFEST_FILENAME), &manifest).unwrap();
        let db = rusqlite::Connection::open(root.join(DATABASE_FILENAME)).unwrap();
        db.execute_batch(include_str!("../migrations/001_initial.sql"))
            .unwrap();
        db.execute_batch(include_str!("../migrations/002_assets.sql"))
            .unwrap();
        db.execute(
            "INSERT INTO assets (asset_id, project_id, original_filename, managed_master_path, file_size, checksum, imported_at_ms, status) VALUES ('asset-1', ?1, 'kept.png', 'assets/masters/kept.png', 4, 'abcd', 1, 'ready')",
            [&manifest.project_id],
        ).unwrap();
        drop(db);

        let project = ProjectState::open(&root).unwrap();
        let (assets, jobs, events, version, source): (i64, i64, i64, i64, String) = project
            .with_db(|db| {
                Ok((
                    db.query_row(
                        "SELECT COUNT(*) FROM assets WHERE asset_id='asset-1'",
                        [],
                        |row| row.get(0),
                    )?,
                    db.query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))?,
                    db.query_row("SELECT COUNT(*) FROM job_events", [], |row| row.get(0))?,
                    db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                        row.get(0)
                    })?,
                    db.query_row(
                        "SELECT source_type FROM assets WHERE asset_id='asset-1'",
                        [],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(
            (assets, jobs, events, version, source.as_str()),
            (1, 0, 0, 11, "imported")
        );
    }

    #[test]
    fn provider_job_migration_preserves_phase4_history_and_assets() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("Phase4Project");
        fs::create_dir_all(&root).unwrap();
        let manifest = ProjectManifest::new("Phase 4".into());
        save_manifest(&root.join(MANIFEST_FILENAME), &manifest).unwrap();
        let db = rusqlite::Connection::open(root.join(DATABASE_FILENAME)).unwrap();
        db.execute_batch(include_str!("../migrations/001_initial.sql"))
            .unwrap();
        db.execute_batch(include_str!("../migrations/002_assets.sql"))
            .unwrap();
        db.execute_batch(include_str!("../migrations/003_jobs.sql"))
            .unwrap();
        db.execute(
            "INSERT INTO assets (asset_id, project_id, original_filename, managed_master_path, file_size, checksum, imported_at_ms, status) VALUES ('asset-1', ?1, 'kept.png', 'assets/masters/kept.png', 4, 'abcd', 1, 'ready')",
            [&manifest.project_id],
        ).unwrap();
        db.execute("INSERT INTO jobs (job_id, job_type, status, payload_json, created_at_ms, updated_at_ms, max_attempts) VALUES ('job-1', 'diagnostic.delay', 'completed', '{\"durationMs\":10,\"failureMode\":\"none\",\"maxAttempts\":1}', 1, 2, 1)", []).unwrap();
        db.execute("INSERT INTO job_events (job_id, event_type, to_status, attempt_count, created_at_ms) VALUES ('job-1', 'completed', 'completed', 1, 2)", []).unwrap();
        drop(db);

        let project = ProjectState::open(&root).unwrap();
        let preserved: (i64, i64, i64, i64) = project.with_db(|db| Ok((
            db.query_row("SELECT COUNT(*) FROM assets WHERE asset_id='asset-1'", [], |row| row.get(0))?,
            db.query_row("SELECT COUNT(*) FROM jobs WHERE job_id='job-1' AND job_type='diagnostic.delay'", [], |row| row.get(0))?,
            db.query_row("SELECT COUNT(*) FROM job_events WHERE job_id='job-1'", [], |row| row.get(0))?,
            db.query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))?,
        ))).unwrap();
        assert_eq!(preserved, (1, 1, 1, 11));
        project.with_db(|db| {
            db.execute("INSERT INTO jobs (job_id, job_type, status, payload_json, created_at_ms, updated_at_ms, max_attempts) VALUES ('job-2', 'provider.diagnostic', 'queued', '{}', 3, 3, 1)", [])?;
            Ok(())
        }).unwrap();
    }

    #[test]
    fn video_migration_preserves_generated_image_job_events_and_provenance() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("Phase6Project");
        fs::create_dir_all(&root).unwrap();
        let manifest = ProjectManifest::new("Phase 6".into());
        save_manifest(&root.join(MANIFEST_FILENAME), &manifest).unwrap();
        let db = rusqlite::Connection::open(root.join(DATABASE_FILENAME)).unwrap();
        for migration in [
            include_str!("../migrations/001_initial.sql"),
            include_str!("../migrations/002_assets.sql"),
            include_str!("../migrations/003_jobs.sql"),
            include_str!("../migrations/004_provider_diagnostic_jobs.sql"),
            include_str!("../migrations/005_image_generation.sql"),
        ] {
            db.execute_batch(migration).unwrap();
        }
        db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts) VALUES ('image-job','image.generate','completed','{}',1,2,2)", []).unwrap();
        db.execute("INSERT INTO job_events (job_id,event_type,to_status,attempt_count,created_at_ms) VALUES ('image-job','completed','completed',1,2)", []).unwrap();
        db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,image_width,image_height,image_format,imported_at_ms,status,source_type) VALUES ('image-asset',?1,'generated.png','assets/masters/generated.png',4,'abcd',64,64,'PNG',2,'ready','generated')", [&manifest.project_id]).unwrap();
        db.execute("INSERT INTO asset_provenance (asset_id,source_type,provider_id,provider_version,prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) VALUES ('image-asset','generated','local.a1111','1.0.0','fixture',7,1,'{}','image-job',2)", []).unwrap();
        drop(db);

        let project = ProjectState::open(&root).unwrap();
        let preserved: (i64, i64, i64, String, Option<i64>, i64) = project.with_db(|db| Ok((
            db.query_row("SELECT COUNT(*) FROM jobs WHERE job_id='image-job'", [], |row| row.get(0))?,
            db.query_row("SELECT COUNT(*) FROM job_events WHERE job_id='image-job'", [], |row| row.get(0))?,
            db.query_row("SELECT COUNT(*) FROM asset_provenance WHERE asset_id='image-asset' AND generating_job_id='image-job'", [], |row| row.get(0))?,
            db.query_row("SELECT media_kind FROM assets WHERE asset_id='image-asset'", [], |row| row.get(0))?,
            db.query_row("SELECT media_width FROM assets WHERE asset_id='image-asset'", [], |row| row.get(0))?,
            db.query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0))?,
        ))).unwrap();
        assert_eq!(preserved, (1, 1, 1, "image".into(), Some(64), 11));
    }

    #[test]
    fn model3d_migration_preserves_video_assets_jobs_events_and_provenance() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("Phase7Project");
        fs::create_dir_all(&root).unwrap();
        let manifest = ProjectManifest::new("Phase 7".into());
        save_manifest(&root.join(MANIFEST_FILENAME), &manifest).unwrap();
        let db = rusqlite::Connection::open(root.join(DATABASE_FILENAME)).unwrap();
        for migration in [
            include_str!("../migrations/001_initial.sql"),
            include_str!("../migrations/002_assets.sql"),
            include_str!("../migrations/003_jobs.sql"),
            include_str!("../migrations/004_provider_diagnostic_jobs.sql"),
            include_str!("../migrations/005_image_generation.sql"),
            include_str!("../migrations/006_video_generation.sql"),
        ] {
            db.execute_batch(migration).unwrap();
        }
        db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,created_at_ms,updated_at_ms,max_attempts) VALUES ('video-job','video.generate','completed','{}',1,2,1)", []).unwrap();
        db.execute("INSERT INTO job_events (job_id,event_type,to_status,attempt_count,created_at_ms) VALUES ('video-job','completed','completed',1,2)", []).unwrap();
        db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,source_type,media_kind,media_container,media_format,validation_level) VALUES ('video-asset',?1,'generated.mp4','assets/masters/generated.mp4',4,'abcd',2,'ready','generated','video','MP4','H.264','structural')", [&manifest.project_id]).unwrap();
        db.execute("INSERT INTO asset_provenance (asset_id,source_type,provider_id,provider_version,prompt,actual_seed,generation_settings_version,generation_settings_json,generating_job_id,generated_at_ms) VALUES ('video-asset','generated','local.comfyui','1.0.0','fixture',7,1,'{}','video-job',2)", []).unwrap();
        drop(db);
        let project = ProjectState::open(&root).unwrap();
        let preserved: (String, String, i64, i64, i64) = project.with_db(|db| db.query_row("SELECT media_kind,media_container,(SELECT COUNT(*) FROM jobs WHERE job_id='video-job'),(SELECT COUNT(*) FROM job_events WHERE job_id='video-job'),(SELECT COUNT(*) FROM asset_provenance WHERE asset_id=assets.asset_id) FROM assets WHERE asset_id='video-asset'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))).unwrap();
        assert_eq!(preserved, ("video".into(), "MP4".into(), 1, 1, 1));
        project.with_db(|db| {
            db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind,model_metadata_schema_version,model_metadata_json) VALUES ('model',?1,'model.glb','assets/masters/model.glb',4,'efgh',3,'ready','model3d',1,'{}')", [&manifest.project_id])?;
            db.execute("INSERT INTO jobs (job_id,job_type,status,payload_json,payload_version,created_at_ms,updated_at_ms,max_attempts) VALUES ('model-job','model3d.generate','queued','{}',1,3,3,1)", [])?;
            Ok(())
        }).unwrap();
    }
}

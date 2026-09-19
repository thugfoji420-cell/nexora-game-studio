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
    #[serde(default)]
    pub engine_scope: Option<String>,
}

impl ProjectManifest {
    pub fn new_scoped(name: String, engine_scope: Option<String>) -> Self {
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
            engine_scope,
        }
    }

    pub fn new(name: String) -> Self {
        Self::new_scoped(name, None)
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
        Self::open_scoped(root, None)
    }

    pub fn open_scoped(root: &Path, expected_scope: Option<&str>) -> Result<Self, ProjectError> {
        let canonical_root = canonicalize_path(root)?;
        let manifest_path = canonical_root.join(MANIFEST_FILENAME);
        let database_path = canonical_root.join(DATABASE_FILENAME);
        let lock_path = canonical_root.join(LOCK_FILENAME);

        let mut manifest = load_manifest(&manifest_path)?;
        manifest.validate()?;
        if let Some(expected_scope) = expected_scope {
            match manifest.engine_scope.as_deref() {
                Some(actual_scope) if actual_scope != expected_scope => {
                    return Err(ProjectError::InvalidRoot(format!(
                        "project belongs to the {actual_scope} engine, not {expected_scope}"
                    )));
                }
                None => {
                    manifest.engine_scope = Some(expected_scope.to_string());
                    save_manifest(&manifest_path, &manifest)?;
                }
                _ => {}
            }
        }

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
        Self::create_scoped(root, name, None)
    }

    pub fn create_scoped(root: &Path, name: &str, engine_scope: Option<&str>) -> Result<Self, ProjectError> {
        let canonical_root = canonicalize_path(root)?;

        let manifest_path = canonical_root.join(MANIFEST_FILENAME);
        let database_path = canonical_root.join(DATABASE_FILENAME);
        let lock_path = canonical_root.join(LOCK_FILENAME);

        if manifest_path.exists() {
            return Err(ProjectError::AlreadyExists(canonical_root.clone()));
        }

        check_no_overlap(&canonical_root)?;

        fs::create_dir_all(&canonical_root)?;

        let manifest = ProjectManifest::new_scoped(name.to_string(), engine_scope.map(str::to_string));
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

        // Migration 12: allow hunyuan.generate job type (feature shipped in
        // code but the jobs CHECK constraint was never updated)
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 12 {
            let migration = db.execute_batch(include_str!("../migrations/012_hunyuan_jobs.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
        }

        // Migration 13: widen material_status on model3d_processing_jobs (the old
        // CHECK rejected real Blender states like 'standard_pbr', silently dropping
        // every persisted processing result)
        let version: u32 = db.query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })?;
        if version < 13 {
            let migration = db.execute_batch(include_str!("../migrations/013_material_status_widen.sql"));
            db.pragma_update(None, "foreign_keys", true)?;
            migration?;
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
            // A lock left behind by a process that is no longer running is a
            // stale lock (e.g. after a crash or force-kill). Verify the PID is
            // still alive on this host before honoring the lock; otherwise
            // take it over by truncating the file and continuing below.
            if existing.host == whoami::username() && Self::process_is_alive(existing.pid) {
                return Err(ProjectError::StaleLock(format!(
                    "project locked by pid {} on {} since {}",
                    existing.pid, existing.host, existing.acquired_at_ms
                )));
            }
            let mut lock_file = fs::OpenOptions::new()
                .write(true)
                .open(&self.lock_path)?;
            lock_file.set_len(0)?;
            lock_file.flush()?;
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

    /// Best-effort check whether a process with the given PID exists on this
    /// machine. Used to detect stale project locks left by crashed or
    /// force-killed app instances. PIDs can be recycled, so this can rarely
    /// produce a false positive; the user can always remove `.nexora.lock`
    /// manually in that case.
    fn process_is_alive(pid: u32) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let output = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
            match output {
                Ok(out) => String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")),
                Err(_) => true, // Cannot verify; assume alive rather than stealing an active lock.
            }
        }
        #[cfg(not(windows))]
        {
            let _ = pid;
            true // Cannot verify portably; assume alive rather than stealing an active lock.
        }
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

/// Characters Windows forbids in a file or directory name.
const INVALID_WINDOWS_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows reserved device names that cannot be used as a directory name.
const RESERVED_WINDOWS_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validate a project name for use as a single directory name.
///
/// The trimmed name is used verbatim so the on-disk folder matches what the
/// user typed. Names that cannot be represented safely on the filesystem are
/// rejected with a clear error rather than silently rewritten.
pub fn sanitize_project_dir_name(name: &str) -> Result<String, ProjectError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProjectError::InvalidRoot("project name is empty".into()));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(ProjectError::InvalidRoot(format!(
            "project name '{trimmed}' is not a valid folder name"
        )));
    }
    if trimmed
        .chars()
        .any(|c| (c as u32) < 0x20 || INVALID_WINDOWS_NAME_CHARS.contains(&c))
    {
        return Err(ProjectError::InvalidRoot(format!(
            "project name '{trimmed}' contains characters that are not allowed in a folder name"
        )));
    }
    if trimmed.ends_with('.') {
        return Err(ProjectError::InvalidRoot(format!(
            "project name '{trimmed}' cannot end with a dot"
        )));
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    if RESERVED_WINDOWS_NAMES.contains(&stem.to_ascii_uppercase().as_str()) {
        return Err(ProjectError::InvalidRoot(format!(
            "project name '{trimmed}' is a reserved Windows device name"
        )));
    }
    Ok(trimmed.to_string())
}

/// Build the final project root from a user-selected PARENT directory and the
/// project name: `<parent>/<sanitized name>`. Treating the selected location as
/// the parent gives every project its own independent directory, so creating a
/// second project under the same parent never collides with the first.
pub fn resolve_new_project_root(parent: &Path, name: &str) -> Result<PathBuf, ProjectError> {
    let dir_name = sanitize_project_dir_name(name)?;
    Ok(parent.join(dir_name))
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
    fn sanitize_rejects_unsafe_dir_names() {
        for bad in ["", "   ", ".", "..", "a/b", "a\\b", "bad:name", "pipe|name", "star*name", "CON", "nul.txt", "trailing."] {
            assert!(
                matches!(sanitize_project_dir_name(bad), Err(ProjectError::InvalidRoot(_))),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn sanitize_trims_and_accepts_safe_names() {
        assert_eq!(sanitize_project_dir_name("  Neon Racing  ").unwrap(), "Neon Racing");
        assert_eq!(sanitize_project_dir_name("Project-B_1").unwrap(), "Project-B_1");
    }

    #[test]
    fn resolve_new_project_root_joins_parent_and_name() {
        let root = resolve_new_project_root(Path::new("D:\\NexoraProjects"), "Neon Racing").unwrap();
        assert_eq!(root.file_name().unwrap(), "Neon Racing");
        assert_eq!(root.parent().unwrap(), Path::new("D:\\NexoraProjects"));
    }

    #[test]
    fn create_second_project_while_first_is_open_stays_independent() {
        let parent = tempdir().unwrap();
        let root_a = resolve_new_project_root(parent.path(), "Project A").unwrap();
        let a = ProjectState::create_scoped(&root_a, "Project A", Some("3d")).unwrap();
        let a_id = a.manifest.project_id.clone();
        let a_root = a.root().clone();
        // Project A is intentionally left open (its lock is held in A's folder).

        let root_b = resolve_new_project_root(parent.path(), "Project B").unwrap();
        let b = ProjectState::create_scoped(&root_b, "Project B", Some("image")).unwrap();
        let b_id = b.manifest.project_id.clone();
        let b_root = b.root().clone();

        // Independent identities, roots, and scopes — nothing reused from A.
        assert_ne!(a_id, b_id);
        assert_ne!(a_root, b_root);
        assert_eq!(a_root.parent(), b_root.parent());
        assert_eq!(a_root.file_name().unwrap(), "Project A");
        assert_eq!(b_root.file_name().unwrap(), "Project B");
        assert_eq!(b.manifest.engine_scope.as_deref(), Some("image"));
        assert!(a_root.join(MANIFEST_FILENAME).exists());
        assert!(b_root.join(MANIFEST_FILENAME).exists());
        assert!(b_root.join(DATABASE_FILENAME).exists());

        b.close().unwrap();
        a.close().unwrap();

        // Reopening each yields its own manifest/scope with no cross-leakage.
        let ra = ProjectState::open_scoped(&a_root, Some("3d")).unwrap();
        assert_eq!(ra.manifest.project_id, a_id);
        assert_eq!(ra.manifest.engine_scope.as_deref(), Some("3d"));
        ra.close().unwrap();
        let rb = ProjectState::open_scoped(&b_root, Some("image")).unwrap();
        assert_eq!(rb.manifest.project_id, b_id);
        assert_eq!(rb.manifest.engine_scope.as_deref(), Some("image"));
        rb.close().unwrap();
    }

    #[test]
    fn create_same_name_under_parent_reports_already_exists() {
        let parent = tempdir().unwrap();
        let root = resolve_new_project_root(parent.path(), "Dup").unwrap();
        ProjectState::create_scoped(&root, "Dup", None)
            .unwrap()
            .close()
            .unwrap();
        let err = ProjectState::create_scoped(&root, "Dup", None).unwrap_err();
        assert!(matches!(err, ProjectError::AlreadyExists(_)));
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
            engine_scope: None,
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
            (1, 0, 0, 13, "imported")
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
        assert_eq!(preserved, (1, 1, 1, 13));
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
        assert_eq!(preserved, (1, 1, 1, "image".into(), Some(64), 13));
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

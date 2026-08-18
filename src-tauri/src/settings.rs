use crate::runtime_manager::{RuntimeConfig, RuntimeKind, RuntimeType};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, io, path::Path, thread, time::Duration};

/// File lock for preventing concurrent access to settings file on Windows
struct SettingsLock {
    lock_path: std::path::PathBuf,
}

impl SettingsLock {
    fn new(path: &Path) -> Self {
        let lock_path = path.with_extension("json.lock");
        Self { lock_path }
    }

    fn acquire(&self) -> io::Result<()> {
        // Ensure parent directory of lock file exists
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Try to create the lock file atomically
        // On Windows, File::create_new fails if the file already exists
        for _ in 0..50 {
            match fs::File::create_new(&self.lock_path) {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    // Lock is held by another process, wait and retry
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Could not acquire settings lock after 50 attempts",
        ))
    }

    fn release(&self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

const SETTINGS_SCHEMA_VERSION: u32 = 1;

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A1111Config {
    pub install_path: Option<String>,
    pub launcher_path: Option<String>,
    pub base_url: String,
    pub auto_start: bool,
    pub startup_args: Vec<String>,
    pub working_directory: Option<String>,
}

impl Default for A1111Config {
    fn default() -> Self {
        Self {
            install_path: None,
            launcher_path: None,
            base_url: "http://127.0.0.1:7860".to_string(),
            auto_start: true,
            startup_args: vec!["--api".to_string(), "--medvram".to_string()],
            working_directory: None,
        }
    }
}

impl A1111Config {
    pub fn to_runtime_config(&self) -> RuntimeConfig {
        RuntimeConfig {
            runtime_id: "automatic1111".to_string(),
            display_name: "Automatic1111".to_string(),
            runtime_type: RuntimeType::Automatic1111,
            kind: RuntimeKind::LongRunningService,
            install_path: self.install_path.clone(),
            launcher_path: self.launcher_path.clone(),
            base_url: Some(self.base_url.clone()),
            health_endpoint: Some("/sdapi/v1/options".to_string()),
            auto_start: self.auto_start,
            startup_args: self.startup_args.clone(),
            working_directory: self.working_directory.clone(),
            environment: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HunyuanConfig {
    pub root_path: Option<String>,
    pub python_executable: Option<String>,
    pub server_entrypoint: Option<String>,
    pub base_url: String,
    pub auto_start: bool,
    pub concurrency: u32,
    pub texture_generation: bool,
}

impl Default for HunyuanConfig {
    fn default() -> Self {
        Self {
            root_path: None,
            python_executable: None,
            server_entrypoint: None,
            base_url: "http://127.0.0.1:8081".to_string(),
            auto_start: true,
            concurrency: 1,
            texture_generation: false,
        }
    }
}

impl HunyuanConfig {
    pub fn to_runtime_config(&self) -> RuntimeConfig {
        let mut args = vec![
            self.server_entrypoint
                .as_deref()
                .unwrap_or("api_server.py")
                .to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "8081".to_string(),
            "--limit-model-concurrency".to_string(),
            self.concurrency.to_string(),
        ];
        if self.texture_generation {
            args.push("--enable_tex".to_string());
        }

        RuntimeConfig {
            runtime_id: "hunyuan3d".to_string(),
            display_name: "Hunyuan3D".to_string(),
            runtime_type: RuntimeType::Hunyuan3D,
            kind: RuntimeKind::LongRunningService,
            install_path: self.root_path.clone(),
            launcher_path: self.python_executable.clone(),
            base_url: Some(self.base_url.clone()),
            health_endpoint: Some("/docs".to_string()),
            auto_start: self.auto_start,
            startup_args: args,
            working_directory: self.root_path.clone(),
            environment: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlenderConfig {
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub validated_at_ms: Option<u128>,
}

impl Default for BlenderConfig {
    fn default() -> Self {
        Self {
            executable_path: None,
            version: None,
            validated_at_ms: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnityProjectTarget {
    pub target_id: String,
    pub display_name: String,
    pub project_root: String,
    pub unity_version: Option<String>,
    pub render_pipeline: Option<String>,
    pub destination_root: Option<String>,
    pub last_validated_at_ms: Option<u128>,
    pub validation_status: Option<String>,
    pub validation_error: Option<String>,
    pub glb_importer_available: bool,
    pub fbx_native_support: bool,
    pub editor_automation_available: bool,
    pub preferred_destination: Option<String>,
    pub material_compatibility: Option<String>,
    pub prefab_capability: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

impl UnityProjectTarget {
    pub fn new(display_name: String, project_root: String) -> Self {
        Self {
            target_id: uuid::Uuid::now_v7().to_string(),
            display_name,
            project_root,
            unity_version: None,
            render_pipeline: None,
            destination_root: Some("Assets/Nexora/ApprovedVehicles".to_string()),
            last_validated_at_ms: None,
            validation_status: Some("unknown".to_string()),
            validation_error: None,
            glb_importer_available: false,
            fbx_native_support: true,
            editor_automation_available: false,
            preferred_destination: None,
            material_compatibility: None,
            prefab_capability: true,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        }
    }

    pub fn with_id(target_id: String, display_name: String, project_root: String) -> Self {
        Self {
            target_id,
            display_name,
            project_root,
            unity_version: None,
            render_pipeline: None,
            destination_root: Some("Assets/Nexora/ApprovedVehicles".to_string()),
            last_validated_at_ms: None,
            validation_status: Some("unknown".to_string()),
            validation_error: None,
            glb_importer_available: false,
            fbx_native_support: true,
            editor_automation_available: false,
            preferred_destination: None,
            material_compatibility: None,
            prefab_capability: true,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub theme: ThemePreference,
    pub compact_sidebar: bool,
    pub telemetry_enabled: bool,
    #[serde(default)]
    pub a1111: A1111Config,
    #[serde(default)]
    pub hunyuan: HunyuanConfig,
    #[serde(default)]
    pub blender: BlenderConfig,
    #[serde(default)]
    pub unity_targets: HashMap<String, UnityProjectTarget>,
    #[serde(default)]
    pub active_unity_target: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Dark,
    System,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            theme: ThemePreference::Dark,
            compact_sidebar: false,
            telemetry_enabled: false,
            a1111: A1111Config::default(),
            hunyuan: HunyuanConfig::default(),
            blender: BlenderConfig::default(),
            unity_targets: HashMap::new(),
            active_unity_target: None,
        }
    }
}

pub fn load(path: &Path) -> io::Result<AppSettings> {
    let lock = SettingsLock::new(path);
    lock.acquire()?;

    let result = (|| {
        if !path.exists() {
            let defaults = AppSettings::default();
            // Release lock before calling save to avoid deadlock
            lock.release();
            save(path, &defaults)?;
            return Ok(defaults);
        }

        // Retry reading the file up to 5 times with increasing delay
        // to handle potential race conditions during startup
        for attempt in 0..5 {
            let content = fs::read(path)?;
            if content.is_empty() {
                // File exists but is empty - return defaults and overwrite with defaults
                let defaults = AppSettings::default();
                lock.release();
                save(path, &defaults)?;
                return Ok(defaults);
            }

            // Handle whitespace-only files
            let content_str = String::from_utf8_lossy(&content);
            let trimmed = content_str.trim();
            if trimmed.is_empty() {
                // File contains only whitespace - treat as empty
                let defaults = AppSettings::default();
                lock.release();
                save(path, &defaults)?;
                return Ok(defaults);
            }

            match serde_json::from_str(trimmed) {
                Ok(settings) => {
                    validate(&settings)?;
                    return Ok(settings);
                }
                Err(e) => {
                    if attempt < 4 {
                        // Wait with exponential backoff and retry
                        thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                        continue;
                    }
                    return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                }
            }
        }

        // Should not reach here, but just in case
        let defaults = AppSettings::default();
        lock.release();
        save(path, &defaults)?;
        Ok(defaults)
    })();

    // Only release if not already released
    if fs::metadata(&lock.lock_path).is_ok() {
        lock.release();
    }
    result
}

pub fn save(path: &Path, settings: &AppSettings) -> io::Result<()> {
    let lock = SettingsLock::new(path);
    lock.acquire()?;

    let result = (|| {
        validate(settings)?;
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "settings path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(temporary, path)
    })();

    lock.release();
    result
}

fn validate(settings: &AppSettings) -> io::Result<()> {
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported settings schema version",
        ));
    }
    if settings.telemetry_enabled {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "telemetry cannot be enabled in Phase 1",
        ));
    }
    // Validate Unity targets
    for (target_id, target) in &settings.unity_targets {
        if target.project_root.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Unity project target '{}' has empty project root",
                    target_id
                ),
            ));
        }
        if let Some(ref dest) = target.destination_root {
            if dest.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Unity project target '{}' has empty destination root",
                        target_id
                    ),
                ));
            }
            // Prevent path traversal
            if dest.contains("..") || dest.starts_with('/') || dest.starts_with('\\') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Unity project target '{}' destination root must be a relative path without traversal sequences",
                        target_id
                    ),
                ));
            }
        }
        if let Some(ref pref_dest) = target.preferred_destination {
            if pref_dest.trim().is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Unity project target '{}' has empty preferred destination",
                        target_id
                    ),
                ));
            }
            if pref_dest.contains("..") || pref_dest.starts_with('/') || pref_dest.starts_with('\\')
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Unity project target '{}' preferred destination must be a relative path without traversal sequences",
                        target_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_local_first_and_telemetry_off() {
        let settings = AppSettings::default();
        assert_eq!(settings.schema_version, 1);
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert!(!settings.compact_sidebar);
        assert!(!settings.telemetry_enabled);
        assert!(settings.blender.executable_path.is_none());
        assert!(settings.blender.version.is_none());
        assert!(settings.unity_targets.is_empty());
        assert!(settings.active_unity_target.is_none());
    }

    #[test]
    fn settings_persist_between_loads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config").join("settings.json");
        let mut settings = load(&path).unwrap();
        settings.compact_sidebar = true;
        settings.theme = ThemePreference::System;
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn test_load_empty_file_returns_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "").unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn test_load_whitespace_file_returns_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "   \n\t  ").unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn test_load_invalid_json_then_valid_returns_valid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        // First write invalid JSON
        fs::write(&path, "{ invalid }").unwrap();
        // Then overwrite with valid JSON
        let defaults = AppSettings::default();
        save(&path, &defaults).unwrap();
        let settings = load(&path).unwrap();
        assert_eq!(settings, AppSettings::default());
    }

    #[test]
    fn unity_target_validation_rejects_empty_project_root() {
        let mut settings = AppSettings::default();
        settings.unity_targets.insert(
            "bad-target".to_string(),
            UnityProjectTarget::with_id(
                "bad-target".to_string(),
                "Bad Target".to_string(),
                "".to_string(), // empty project root
            ),
        );
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn unity_target_validation_rejects_traversal_in_destination() {
        let mut settings = AppSettings::default();
        settings.unity_targets.insert(
            "bad-target".to_string(),
            UnityProjectTarget::with_id(
                "bad-target".to_string(),
                "Bad Target".to_string(),
                "/valid/path".to_string(),
            ),
        );
        // Manually set a bad destination with traversal
        if let Some(target) = settings.unity_targets.get_mut("bad-target") {
            target.destination_root = Some("../outside".to_string());
        }
        assert!(validate(&settings).is_err());
    }

    #[test]
    fn test_load_old_settings_without_runtime_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");

        // Old settings format without a1111, hunyuan, blender, unity_targets, active_unity_target
        let old_settings_json = r#"{
            "schemaVersion": 1,
            "theme": "dark",
            "compactSidebar": false,
            "telemetryEnabled": false
        }"#;

        fs::write(&path, old_settings_json).unwrap();
        let settings = load(&path).unwrap();

        // Verify load succeeds
        assert_eq!(settings.schema_version, 1);
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert!(!settings.compact_sidebar);
        assert!(!settings.telemetry_enabled);

        // Verify new fields receive safe defaults
        assert_eq!(settings.a1111, A1111Config::default());
        assert_eq!(settings.hunyuan, HunyuanConfig::default());
        assert_eq!(settings.blender, BlenderConfig::default());
        assert!(settings.unity_targets.is_empty());
        assert!(settings.active_unity_target.is_none());

        // Verify saving writes current schema format
        save(&path, &settings).unwrap();
        let saved_content = fs::read_to_string(&path).unwrap();
        let saved_settings: serde_json::Value = serde_json::from_str(&saved_content).unwrap();

        // Check that new fields are present in saved output
        assert!(saved_settings.get("a1111").is_some());
        assert!(saved_settings.get("hunyuan").is_some());
        assert!(saved_settings.get("blender").is_some());
        assert!(saved_settings.get("unityTargets").is_some());
        assert!(saved_settings.get("activeUnityTarget").is_some());

        // Verify reloading the migrated file succeeds
        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded, settings);
    }
}

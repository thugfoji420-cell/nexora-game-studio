use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlenderInfo {
    pub installed: bool,
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub valid: bool,
    pub error: Option<String>,
}

const COMMON_BLENDER_PATHS: &[&str] = &[
    r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 4.1\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 4.0\blender.exe",
    r"C:\Program Files\Blender Foundation\Blender 3.6\blender.exe",
];

pub fn discover_blender() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for path in COMMON_BLENDER_PATHS {
        let p = PathBuf::from(path);
        if p.exists() {
            candidates.push(p);
        }
    }

    if let Ok(program_files) = std::env::var("ProgramFiles") {
        let base = PathBuf::from(program_files).join("Blender Foundation");
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry.path().join("blender.exe");
                if path.exists() && !candidates.iter().any(|c| c == &path) {
                    candidates.push(path);
                }
            }
        }
    }

    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        let base = PathBuf::from(program_files_x86).join("Blender Foundation");
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry.path().join("blender.exe");
                if path.exists() && !candidates.iter().any(|c| c == &path) {
                    candidates.push(path);
                }
            }
        }
    }

    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let base = PathBuf::from(local_app_data).join("Blender Foundation");
        if let Ok(entries) = fs::read_dir(&base) {
            for entry in entries.flatten() {
                let path = entry.path().join("blender.exe");
                if path.exists() && !candidates.iter().any(|c| c == &path) {
                    candidates.push(path);
                }
            }
        }
    }

    candidates
}

pub fn validate_blender(executable: &Path) -> BlenderInfo {
    let mut cmd = Command::new(executable);
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd.output();

    match output {
        Ok(out) => {
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let version = stdout
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .map(|s| s.to_string());

                BlenderInfo {
                    installed: true,
                    executable_path: Some(executable.to_string_lossy().to_string()),
                    version,
                    valid: true,
                    error: None,
                }
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                BlenderInfo {
                    installed: true,
                    executable_path: Some(executable.to_string_lossy().to_string()),
                    version: None,
                    valid: false,
                    error: Some(format!("Blender exited with error: {}", stderr)),
                }
            }
        }
        Err(e) => BlenderInfo {
            installed: true,
            executable_path: Some(executable.to_string_lossy().to_string()),
            version: None,
            valid: false,
            error: Some(format!("Failed to execute Blender: {}", e)),
        },
    }
}

pub fn find_and_validate_blender(manual_path: Option<&Path>) -> BlenderInfo {
    if let Some(path) = manual_path {
        if path.exists() {
            return validate_blender(path);
        } else {
            return BlenderInfo {
                installed: false,
                executable_path: Some(path.to_string_lossy().to_string()),
                version: None,
                valid: false,
                error: Some("Configured Blender executable not found".into()),
            };
        }
    }

    let candidates = discover_blender();
    for candidate in candidates {
        let info = validate_blender(&candidate);
        if info.valid {
            return info;
        }
    }

    BlenderInfo {
        installed: false,
        executable_path: None,
        version: None,
        valid: false,
        error: Some("No valid Blender installation found".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_blender_no_crash() {
        let candidates = discover_blender();
        // Just ensure it doesn't panic
        assert!(
            candidates
                .iter()
                .all(|p| p.to_string_lossy().ends_with("blender.exe"))
        );
    }
}

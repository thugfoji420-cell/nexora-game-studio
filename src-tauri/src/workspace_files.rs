//! Shared boundary for agent and manual code-file commands. This is not an OS sandbox.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: usize = 512 * 1024;
pub const CODE_EXTENSIONS: &[&str] = &[
    "cs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "dart", "kt", "java", "swift", "rs", "go",
    "c", "cpp", "h", "hpp", "html", "css", "scss", "json", "xml", "yml", "yaml", "toml", "md",
    "txt", "sql", "lua", "sh",
];

pub fn validate_relative(rel: &str) -> Result<PathBuf, String> {
    let normalized = rel.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') || normalized.len() > 1024 {
        return Err("expected a non-empty relative file path".into());
    }
    for part in normalized.split('/') {
        let lower = part.to_ascii_lowercase();
        let stem = lower.split('.').next().unwrap_or("");
        if part.is_empty()
            || part.starts_with('.')
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || ":*?\"<>|".contains(c))
            || matches!(stem, "con" | "prn" | "aux" | "nul")
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && stem.as_bytes()[3].is_ascii_digit())
            || matches!(
                lower.as_str(),
                "node_modules"
                    | "target"
                    | "nexora.project.json"
                    | "credentials.json"
                    | "secrets.json"
                    | "service-account.json"
            )
            || lower.starts_with("secrets.")
            || lower.starts_with("credentials.")
        {
            return Err(format!("protected or invalid path component: {part}"));
        }
    }
    let path = PathBuf::from(normalized);
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !CODE_EXTENSIONS.contains(&ext.as_str()) {
        return Err("file type is not allowed for code access".into());
    }
    Ok(path)
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Includes directory junctions and all other reparse points, not just symlinks.
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

/// Allows missing descendants for creation, but validates every existing ancestor.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let relative = validate_relative(rel)?;
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    let mut current = root.clone();
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if is_link(&meta) {
                    return Err("links and reparse points are not allowed".into());
                }
                if !meta.is_dir() && !meta.is_file() {
                    return Err("not a regular file or directory".into());
                }
                let canonical = dunce::canonicalize(&current).map_err(|e| e.to_string())?;
                if !canonical.starts_with(&root) {
                    return Err("path escapes workspace".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(current)
}

pub fn read(root: &Path, rel: &str, limit: usize) -> Result<String, String> {
    let path = resolve(root, rel)?;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("file exceeds {limit} byte read limit"));
    }
    String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text".into())
}

pub fn write(root: &Path, rel: &str, content: &str) -> Result<(), String> {
    if content.trim().is_empty() || content.len() > MAX_FILE_BYTES {
        return Err("file must be non-empty and at most 512 KB".into());
    }
    let target = resolve(root, rel)?;
    let parent = target.parent().ok_or("missing parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Revalidate after creation. Atomic replacement also avoids modifying a hardlink's source.
    let target = resolve(root, rel)?;
    let temp = parent.join(format!(".nexora-write-{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, target).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_platform_escapes_secrets_and_control_files() {
        for name in [
            "../a.ts",
            "src/../../a.ts",
            "/a.ts",
            "C:/a.ts",
            "C:a.ts",
            "\\\\server\\a.ts",
            "src/a.ts:stream",
            "src/./a.ts",
            "src/a.ts.",
            "src/CON.txt",
            "src/NUL.ts",
            ".env.json",
            ".git/config.json",
            ".vscode/tasks.json",
            "nexora.project.json",
            "secrets.json",
            "credentials.json",
            "a\0.ts",
            "node_modules/a.js",
        ] {
            assert!(validate_relative(name).is_err(), "must reject {name:?}");
        }
        assert!(validate_relative("src\\CarController.cs").is_ok());
    }

    #[test]
    fn creates_new_nested_files_and_replaces_existing_files() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "src/new/file.ts", "export const n = 1;").unwrap();
        write(root.path(), "src/new/file.ts", "export const n = 2;").unwrap();
        assert_eq!(
            read(root.path(), "src/new/file.ts", 100).unwrap(),
            "export const n = 2;"
        );
        assert!(read(root.path(), "src/new/file.ts", 4).is_err());
    }

    #[test]
    fn replacing_a_hardlink_does_not_modify_its_source() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("original.ts"), "original").unwrap();
        fs::hard_link(
            outside.path().join("original.ts"),
            root.path().join("linked.ts"),
        )
        .unwrap();
        write(root.path(), "linked.ts", "replacement").unwrap();
        assert_eq!(
            fs::read_to_string(outside.path().join("original.ts")).unwrap(),
            "original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_ancestors_and_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(write(root.path(), "escape/new.ts", "bad").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_junction_ancestors() {
        use std::os::windows::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escape");
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .creation_flags(0x08000000)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "junction creation must succeed to test the boundary"
        );
        assert!(read(root.path(), "escape/file.ts", 100).is_err());
        assert!(write(root.path(), "escape/new.ts", "bad").is_err());
        fs::remove_dir(link).unwrap();
    }
}

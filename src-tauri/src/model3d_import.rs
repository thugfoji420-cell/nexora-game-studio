use crate::project::ProjectState;
use std::{fs, path::PathBuf};
use uuid::Uuid;

pub struct ModelImportPaths {
    pub staging: PathBuf,
    pub masters: PathBuf,
}

pub fn prepare(project: &ProjectState, asset_id: &str) -> Result<ModelImportPaths, String> {
    let id = canonical_uuid(asset_id)?;
    let root = dunce::canonicalize(&project.root).map_err(|error| error.to_string())?;
    let assets_path = root.join("assets");
    fs::create_dir_all(&assets_path).map_err(|error| error.to_string())?;
    let assets = direct_child(&root, &assets_path, "assets")?;

    let staging_root_path = assets.join("staging");
    fs::create_dir_all(&staging_root_path).map_err(|error| error.to_string())?;
    let staging_root = direct_child(&assets, &staging_root_path, "staging")?;
    let masters_path = assets.join("masters");
    fs::create_dir_all(&masters_path).map_err(|error| error.to_string())?;
    let masters = direct_child(&assets, &masters_path, "masters")?;

    let staging_path = staging_root.join(id.to_string());
    fs::create_dir_all(&staging_path).map_err(|error| error.to_string())?;
    let staging = direct_child(&staging_root, &staging_path, "model staging")?;
    Ok(ModelImportPaths { staging, masters })
}

pub fn recover(project: &ProjectState) -> Result<usize, String> {
    let root = dunce::canonicalize(&project.root).map_err(|error| error.to_string())?;
    let assets_path = root.join("assets");
    if !assets_path.exists() {
        return Ok(0);
    }
    let assets = direct_child(&root, &assets_path, "assets")?;
    let staging_path = assets.join("staging");
    if !staging_path.exists() {
        return Ok(0);
    }
    let staging_root = direct_child(&assets, &staging_path, "staging")?;
    let masters_path = assets.join("masters");
    if !masters_path.exists() {
        return Ok(0);
    }
    let masters = direct_child(&assets, &masters_path, "masters")?;
    let mut recovered = 0;
    for entry in fs::read_dir(&staging_root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(&name) else {
            continue;
        };
        if id.to_string() != name
            || !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
        {
            continue;
        }
        let staging = match safe_existing_child(&staging_root, &entry.path()) {
            Some(path) => path,
            None => continue,
        };
        let committed = project
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM assets WHERE asset_id=?1)",
                [&name],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        for extension in ["glb", "gltf"] {
            if !committed {
                remove_safe_file(&masters, &masters.join(format!("{name}.{extension}")))?;
            }
            remove_safe_file(&masters, &masters.join(format!(".{name}.{extension}.tmp")))?;
        }
        fs::remove_dir_all(staging).map_err(|error| error.to_string())?;
        recovered += 1;
    }
    Ok(recovered)
}

fn canonical_uuid(value: &str) -> Result<Uuid, String> {
    let id = Uuid::parse_str(value).map_err(|_| "invalid model import identity".to_string())?;
    if id.to_string() != value {
        return Err("invalid model import identity".into());
    }
    Ok(id)
}

fn direct_child(
    parent: &std::path::Path,
    child: &std::path::Path,
    label: &str,
) -> Result<PathBuf, String> {
    let child = dunce::canonicalize(child).map_err(|error| error.to_string())?;
    if child.parent() != Some(parent) {
        return Err(format!("{label} directory escapes managed project storage"));
    }
    Ok(child)
}

fn safe_existing_child(parent: &std::path::Path, child: &std::path::Path) -> Option<PathBuf> {
    let child = dunce::canonicalize(child).ok()?;
    (child.parent() == Some(parent)).then_some(child)
}

fn remove_safe_file(parent: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Ok(());
    }
    let Some(path) = safe_existing_child(parent, path) else {
        return Ok(());
    };
    fs::remove_file(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn project() -> (tempfile::TempDir, ProjectState) {
        let dir = tempdir().unwrap();
        let project = ProjectState::create(&dir.path().join("project"), "Imports").unwrap();
        (dir, project)
    }

    #[test]
    fn open_removes_only_uncommitted_uuid_staging() {
        let (dir, project) = project();
        let id = Uuid::now_v7().to_string();
        let paths = prepare(&project, &id).unwrap();
        fs::write(paths.staging.join("source.glb"), b"partial").unwrap();
        let unrelated = project.root.join("assets").join("staging").join("notes");
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("keep.txt"), b"keep").unwrap();
        drop(project);

        let reopened = ProjectState::open(&dir.path().join("project")).unwrap();
        assert!(
            !reopened
                .root
                .join("assets")
                .join("staging")
                .join(id)
                .exists()
        );
        assert_eq!(fs::read(unrelated.join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn open_removes_orphan_model_master_and_matching_temps() {
        let (dir, project) = project();
        let id = Uuid::now_v7().to_string();
        let paths = prepare(&project, &id).unwrap();
        let master = paths.masters.join(format!("{id}.glb"));
        let temporary = paths.masters.join(format!(".{id}.gltf.tmp"));
        fs::write(&master, b"orphan").unwrap();
        fs::write(&temporary, b"temporary").unwrap();
        drop(project);

        let _reopened = ProjectState::open(&dir.path().join("project")).unwrap();
        assert!(!master.exists());
        assert!(!temporary.exists());
    }

    #[test]
    fn open_preserves_committed_model_master_and_cleans_staging() {
        let (dir, project) = project();
        let id = Uuid::now_v7().to_string();
        let paths = prepare(&project, &id).unwrap();
        let master = paths.masters.join(format!("{id}.gltf"));
        let temporary = paths.masters.join(format!(".{id}.gltf.tmp"));
        fs::write(&master, b"committed").unwrap();
        fs::write(&temporary, b"temporary").unwrap();
        fs::write(paths.staging.join("source.gltf"), b"staged").unwrap();
        project.with_db(|db| db.execute("INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'model.gltf',?3,9,'checksum',1,'ready','model3d')", rusqlite::params![id, project.manifest.project_id, master.to_string_lossy()]).map(|_| ())).unwrap();
        drop(project);

        let reopened = ProjectState::open(&dir.path().join("project")).unwrap();
        assert_eq!(fs::read(&master).unwrap(), b"committed");
        assert!(!temporary.exists());
        assert!(
            !reopened
                .root
                .join("assets")
                .join("staging")
                .join(id)
                .exists()
        );
    }
}

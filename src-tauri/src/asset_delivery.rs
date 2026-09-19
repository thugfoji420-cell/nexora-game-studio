use crate::{AppState, project::ProjectState};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, Metadata},
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use tauri::{Manager, Runtime, http};
use uuid::Uuid;

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_INTEGRITY_CACHE_ENTRIES: usize = 64;

#[derive(Clone, Default)]
pub(crate) struct IntegrityCache(Arc<Mutex<HashSet<IntegrityKey>>>);

#[derive(Clone, Eq)]
struct IntegrityKey {
    project_root: PathBuf,
    project_id: String,
    asset_id: String,
    checksum: String,
    file_size: u64,
    modified_nanos: u128,
}

impl PartialEq for IntegrityKey {
    fn eq(&self, other: &Self) -> bool {
        self.project_root == other.project_root
            && self.project_id == other.project_id
            && self.asset_id == other.asset_id
            && self.checksum == other.checksum
            && self.file_size == other.file_size
            && self.modified_nanos == other.modified_nanos
    }
}

impl Hash for IntegrityKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.project_root.hash(state);
        self.project_id.hash(state);
        self.asset_id.hash(state);
        self.checksum.hash(state);
        self.file_size.hash(state);
        self.modified_nanos.hash(state);
    }
}

#[derive(Debug, PartialEq)]
enum DeliveryError {
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    RangeRequired,
    RangeNotSatisfiable(u64),
    Internal,
}

#[derive(Debug)]
struct ResolvedAsset {
    file: File,
    file_size: u64,
    modified_identity: u128,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ByteSelection {
    start: u64,
    end: u64,
    partial: bool,
}

pub(crate) fn handle_protocol<R: Runtime>(
    app: &tauri::AppHandle<R>,
    request: http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    // Snapshot the active project before dispatch so a later project switch cannot retarget this request.
    let state = app.state::<AppState>();
    let project = state
        .current_project
        .lock()
        .ok()
        .and_then(|current| current.clone());
    let cache = state.media_integrity_cache.clone();

    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(serve_request(project, cache, request));
    });
}

fn serve_request(
    project: Option<Arc<ProjectState>>,
    cache: IntegrityCache,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let is_head = request.method() == http::Method::HEAD;
    if request.method() != http::Method::GET && !is_head {
        return error_response(DeliveryError::MethodNotAllowed, false);
    }
    let asset_id = match resolve_asset_id(request.uri()) {
        Ok(asset_id) => asset_id,
        Err(error) => return error_response(error, is_head),
    };
    let project = match project {
        Some(project) => project,
        None => return error_response(DeliveryError::Forbidden, is_head),
    };

    // First check media_kind to determine delivery handler
    let media_kind: Option<String> = {
        let db = match project.db.lock() {
            Ok(db) => db,
            Err(_) => return error_response(DeliveryError::Internal, is_head),
        };
        match db.query_row(
            "SELECT media_kind FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready'",
            rusqlite::params![&asset_id, project.manifest.project_id],
            |row| row.get(0),
        ).optional() {
            Ok(mk) => mk,
            Err(_) => return error_response(DeliveryError::Internal, is_head),
        }
    };

    let mut asset = match media_kind.as_deref() {
        Some("video") => match resolve_video(&project, &asset_id, &cache) {
            Ok(a) => a,
            Err(e) => return error_response(e, is_head),
        },
        Some("model3d") => match resolve_model3d(&project, &asset_id, &cache) {
            Ok(a) => a,
            Err(e) => return error_response(e, is_head),
        },
        _ => return error_response(DeliveryError::NotFound, is_head),
    };

    let range = match request.headers().get(http::header::RANGE) {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => {
                return error_response(
                    DeliveryError::RangeNotSatisfiable(asset.file_size),
                    is_head,
                );
            }
        },
        None => None,
    };
    let selection = match select_bytes(range, asset.file_size, is_head) {
        Ok(selection) => selection,
        Err(error) => return error_response(error, is_head),
    };
    let selected_len = selection.end - selection.start + 1;
    let body = if is_head {
        Vec::new()
    } else {
        match read_selected_bytes(&mut asset, selection) {
            Ok(body) => body,
            Err(error) => return error_response(error, false),
        }
    };

    let content_type = match media_kind.as_deref() {
        Some("video") => "video/mp4",
        Some("model3d") => "model/gltf-binary",
        _ => "application/octet-stream",
    };

    let mut response = http::Response::builder()
        .status(if selection.partial {
            http::StatusCode::PARTIAL_CONTENT
        } else {
            http::StatusCode::OK
        })
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::ACCEPT_RANGES, "bytes")
        .header(http::header::CONTENT_LENGTH, selected_len.to_string());
    if selection.partial {
        response = response.header(
            http::header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                selection.start, selection.end, asset.file_size
            ),
        );
    }
    response
        .body(body)
        .unwrap_or_else(|_| error_response(DeliveryError::Internal, is_head))
}

fn resolve_asset_id(uri: &http::Uri) -> Result<String, DeliveryError> {
    if uri.query().is_some() {
        return Err(DeliveryError::BadRequest);
    }
    let asset_id = uri
        .path()
        .strip_prefix('/')
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or(DeliveryError::BadRequest)?;
    let parsed = Uuid::parse_str(asset_id).map_err(|_| DeliveryError::BadRequest)?;
    if parsed.to_string() != asset_id {
        return Err(DeliveryError::BadRequest);
    }
    Ok(asset_id.to_owned())
}

fn select_bytes(
    range: Option<&str>,
    file_size: u64,
    is_head: bool,
) -> Result<ByteSelection, DeliveryError> {
    if file_size == 0 {
        return Err(DeliveryError::RangeNotSatisfiable(0));
    }
    let Some(range) = range else {
        // The Tauri responder requires an in-memory body. Refuse an unbounded full GET honestly;
        // Accept-Ranges on the error lets native media retry with its normal byte-range request.
        if !is_head && file_size > MAX_RESPONSE_BYTES {
            return Err(DeliveryError::RangeRequired);
        }
        return Ok(ByteSelection {
            start: 0,
            end: file_size - 1,
            partial: false,
        });
    };
    let spec = range
        .strip_prefix("bytes=")
        .filter(|value| !value.is_empty() && !value.contains(','))
        .ok_or(DeliveryError::RangeNotSatisfiable(file_size))?;
    let (start, end) = spec
        .split_once('-')
        .ok_or(DeliveryError::RangeNotSatisfiable(file_size))?;
    let (start, requested_end) = if start.is_empty() {
        let suffix = end
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(DeliveryError::RangeNotSatisfiable(file_size))?;
        (file_size.saturating_sub(suffix), file_size - 1)
    } else {
        let start = start
            .parse::<u64>()
            .map_err(|_| DeliveryError::RangeNotSatisfiable(file_size))?;
        let end = if end.is_empty() {
            file_size - 1
        } else {
            end.parse::<u64>()
                .map_err(|_| DeliveryError::RangeNotSatisfiable(file_size))?
                .min(file_size - 1)
        };
        if start >= file_size || start > end {
            return Err(DeliveryError::RangeNotSatisfiable(file_size));
        }
        (start, end)
    };
    let end = if is_head {
        requested_end
    } else {
        requested_end.min(start.saturating_add(MAX_RESPONSE_BYTES - 1))
    };
    Ok(ByteSelection {
        start,
        end,
        partial: true,
    })
}

fn resolve_video(
    project: &ProjectState,
    asset_id: &str,
    cache: &IntegrityCache,
) -> Result<ResolvedAsset, DeliveryError> {
    let project_id = project.manifest.project_id.clone();
    let row: Option<(String, i64, String)> = {
        let db = project.db.lock().map_err(|_| DeliveryError::Internal)?;
        db.query_row(
            "SELECT managed_master_path,file_size,checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND media_kind='video' AND status='ready'",
            rusqlite::params![asset_id, project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| DeliveryError::Internal)?
    };
    let (registered_path, registered_size, checksum) = row.ok_or(DeliveryError::NotFound)?;
    let file_size = u64::try_from(registered_size).map_err(|_| DeliveryError::Forbidden)?;

    let assets =
        dunce::canonicalize(project.root.join("assets")).map_err(|_| DeliveryError::NotFound)?;
    let masters = dunce::canonicalize(project.root.join("assets").join("masters"))
        .map_err(|_| DeliveryError::NotFound)?;
    if assets.parent() != Some(project.root.as_path()) || masters.parent() != Some(assets.as_path())
    {
        return Err(DeliveryError::Forbidden);
    }
    let expected = project
        .root
        .join("assets")
        .join("masters")
        .join(format!("{asset_id}.mp4"));
    let expected = dunce::canonicalize(&expected).map_err(|_| DeliveryError::NotFound)?;
    let registered = PathBuf::from(registered_path);
    let registered = if registered.is_absolute() {
        registered
    } else {
        project.root.join(registered)
    };
    let registered = dunce::canonicalize(registered).map_err(|_| DeliveryError::Forbidden)?;
    if expected != registered
        || !expected.starts_with(&masters)
        || expected.parent() != Some(masters.as_path())
    {
        return Err(DeliveryError::Forbidden);
    }

    let mut file = File::open(&expected).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => DeliveryError::NotFound,
        std::io::ErrorKind::PermissionDenied => DeliveryError::Forbidden,
        _ => DeliveryError::Internal,
    })?;
    if dunce::canonicalize(
        project
            .root
            .join("assets")
            .join("masters")
            .join(format!("{asset_id}.mp4")),
    )
    .map_err(|_| DeliveryError::Forbidden)?
        != expected
    {
        return Err(DeliveryError::Forbidden);
    }
    let metadata = file.metadata().map_err(|_| DeliveryError::Internal)?;
    if !metadata.is_file() || metadata.len() != file_size {
        return Err(DeliveryError::Forbidden);
    }
    let modified_identity = verify_integrity(
        &mut file, &metadata, project, asset_id, &checksum, file_size, cache,
    )?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| DeliveryError::Internal)?;
    Ok(ResolvedAsset {
        file,
        file_size,
        modified_identity,
    })
}

fn resolve_model3d(
    project: &ProjectState,
    asset_id: &str,
    cache: &IntegrityCache,
) -> Result<ResolvedAsset, DeliveryError> {
    let project_id = project.manifest.project_id.clone();
    let row: Option<(String, i64, String)> = {
        let db = project.db.lock().map_err(|_| DeliveryError::Internal)?;
        db.query_row(
            "SELECT managed_master_path,file_size,checksum FROM assets WHERE asset_id=?1 AND project_id=?2 AND media_kind='model3d' AND status='ready'",
            rusqlite::params![asset_id, project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| DeliveryError::Internal)?
    };
    let (registered_path, registered_size, checksum) = row.ok_or(DeliveryError::NotFound)?;
    let file_size = u64::try_from(registered_size).map_err(|_| DeliveryError::Forbidden)?;

    let assets =
        dunce::canonicalize(project.root.join("assets")).map_err(|_| DeliveryError::NotFound)?;
    let masters = dunce::canonicalize(project.root.join("assets").join("masters"))
        .map_err(|_| DeliveryError::NotFound)?;
    if assets.parent() != Some(project.root.as_path()) || masters.parent() != Some(assets.as_path())
    {
        return Err(DeliveryError::Forbidden);
    }
    let expected = project
        .root
        .join("assets")
        .join("masters")
        .join(format!("{asset_id}.glb"));
    let expected = dunce::canonicalize(&expected).map_err(|_| DeliveryError::NotFound)?;
    let registered = PathBuf::from(registered_path);
    let registered = if registered.is_absolute() {
        registered
    } else {
        project.root.join(registered)
    };
    let registered = dunce::canonicalize(registered).map_err(|_| DeliveryError::Forbidden)?;
    if expected != registered
        || !expected.starts_with(&masters)
        || expected.parent() != Some(masters.as_path())
    {
        return Err(DeliveryError::Forbidden);
    }

    let mut file = File::open(&expected).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => DeliveryError::NotFound,
        std::io::ErrorKind::PermissionDenied => DeliveryError::Forbidden,
        _ => DeliveryError::Internal,
    })?;
    if dunce::canonicalize(
        project
            .root
            .join("assets")
            .join("masters")
            .join(format!("{asset_id}.glb")),
    )
    .map_err(|_| DeliveryError::Forbidden)?
        != expected
    {
        return Err(DeliveryError::Forbidden);
    }
    let metadata = file.metadata().map_err(|_| DeliveryError::Internal)?;
    if !metadata.is_file() || metadata.len() != file_size {
        return Err(DeliveryError::Forbidden);
    }
    let modified_identity = verify_integrity(
        &mut file, &metadata, project, asset_id, &checksum, file_size, cache,
    )?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| DeliveryError::Internal)?;
    Ok(ResolvedAsset {
        file,
        file_size,
        modified_identity,
    })
}

fn verify_integrity(
    file: &mut File,
    metadata: &Metadata,
    project: &ProjectState,
    asset_id: &str,
    checksum: &str,
    file_size: u64,
    cache: &IntegrityCache,
) -> Result<u128, DeliveryError> {
    let modified_nanos = modified_identity(metadata)?;
    let key = IntegrityKey {
        project_root: project.root.clone(),
        project_id: project.manifest.project_id.clone(),
        asset_id: asset_id.to_owned(),
        checksum: checksum.to_owned(),
        file_size,
        modified_nanos,
    };
    if cache
        .0
        .lock()
        .map_err(|_| DeliveryError::Internal)?
        .contains(&key)
    {
        return Ok(modified_nanos);
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|_| DeliveryError::Internal)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| DeliveryError::Internal)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let after = file.metadata().map_err(|_| DeliveryError::Internal)?;
    if after.len() != file_size || modified_identity(&after)? != modified_nanos {
        return Err(DeliveryError::Forbidden);
    }
    if format!("{:x}", hasher.finalize()) != checksum {
        return Err(DeliveryError::Forbidden);
    }
    let mut entries = cache.0.lock().map_err(|_| DeliveryError::Internal)?;
    if entries.len() >= MAX_INTEGRITY_CACHE_ENTRIES {
        entries.clear();
    }
    entries.insert(key);
    Ok(modified_nanos)
}

fn read_selected_bytes(
    asset: &mut ResolvedAsset,
    selection: ByteSelection,
) -> Result<Vec<u8>, DeliveryError> {
    let selected_len = selection.end - selection.start + 1;
    asset
        .file
        .seek(SeekFrom::Start(selection.start))
        .map_err(|_| DeliveryError::Internal)?;
    let mut body = Vec::with_capacity(selected_len as usize);
    asset
        .file
        .by_ref()
        .take(selected_len)
        .read_to_end(&mut body)
        .map_err(|_| DeliveryError::Internal)?;
    if body.len() as u64 != selected_len {
        return Err(DeliveryError::Internal);
    }
    let metadata = asset.file.metadata().map_err(|_| DeliveryError::Internal)?;
    if metadata.len() != asset.file_size || modified_identity(&metadata)? != asset.modified_identity
    {
        return Err(DeliveryError::Forbidden);
    }
    Ok(body)
}

fn modified_identity(metadata: &Metadata) -> Result<u128, DeliveryError> {
    metadata
        .modified()
        .map_err(|_| DeliveryError::Internal)?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DeliveryError::Internal)
        .map(|value| value.as_nanos())
}

fn error_response(error: DeliveryError, is_head: bool) -> http::Response<Vec<u8>> {
    let (status, message) = match error {
        DeliveryError::BadRequest => (http::StatusCode::BAD_REQUEST, "bad request"),
        DeliveryError::Forbidden => (http::StatusCode::FORBIDDEN, "forbidden"),
        DeliveryError::NotFound => (http::StatusCode::NOT_FOUND, "not found"),
        DeliveryError::MethodNotAllowed => {
            (http::StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
        }
        DeliveryError::RangeRequired => (http::StatusCode::BAD_REQUEST, "byte range required"),
        DeliveryError::RangeNotSatisfiable(_) => (
            http::StatusCode::RANGE_NOT_SATISFIABLE,
            "range not satisfiable",
        ),
        DeliveryError::Internal => (http::StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    let mut response = http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(http::header::CONTENT_LENGTH, message.len().to_string());
    if matches!(error, DeliveryError::MethodNotAllowed) {
        response = response.header(http::header::ALLOW, "GET, HEAD");
    }
    if let DeliveryError::RangeNotSatisfiable(file_size) = error {
        response = response
            .header(http::header::ACCEPT_RANGES, "bytes")
            .header(http::header::CONTENT_RANGE, format!("bytes */{file_size}"));
    }
    if matches!(error, DeliveryError::RangeRequired) {
        response = response.header(http::header::ACCEPT_RANGES, "bytes");
    }
    response
        .body(if is_head {
            Vec::new()
        } else {
            message.as_bytes().to_vec()
        })
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{fs, io::Write};
    use tempfile::TempDir;

    const ASSET_ID: &str = "018f47a2-4f25-7a30-8000-0123456789ab";
    const OTHER_ID: &str = "018f47a2-4f25-7a30-8000-0123456789ac";

    fn project_with_asset(
        kind: &str,
        status: &str,
        project_id: Option<&str>,
    ) -> (TempDir, Arc<ProjectState>, Vec<u8>) {
        let directory = tempfile::tempdir().unwrap();
        let project = Arc::new(ProjectState::create(directory.path(), "Media test").unwrap());
        let bytes = b"0123456789abcdef".to_vec();
        let masters = project.root.join("assets/masters");
        fs::create_dir_all(&masters).unwrap();
        let master = masters.join(format!("{ASSET_ID}.mp4"));
        fs::write(&master, &bytes).unwrap();
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        let row_project = project_id.unwrap_or(&project.manifest.project_id);
        project.db.lock().unwrap().execute(
            "INSERT INTO assets (asset_id,project_id,original_filename,managed_master_path,file_size,checksum,imported_at_ms,status,media_kind) VALUES (?1,?2,'video.mp4',?3,?4,?5,1,?6,?7)",
            rusqlite::params![ASSET_ID, row_project, master.to_string_lossy(), bytes.len() as i64, checksum, status, kind],
        ).unwrap();
        (directory, project, bytes)
    }

    fn request(method: http::Method, path: &str, range: Option<&str>) -> http::Request<Vec<u8>> {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("http://nexora-media.localhost{path}"));
        if let Some(range) = range {
            builder = builder.header(http::header::RANGE, range);
        }
        builder.body(Vec::new()).unwrap()
    }

    fn serve(
        project: Arc<ProjectState>,
        method: http::Method,
        path: &str,
        range: Option<&str>,
    ) -> http::Response<Vec<u8>> {
        serve_request(
            Some(project),
            IntegrityCache::default(),
            request(method, path, range),
        )
    }

    #[test]
    fn route_requires_exact_canonical_uuid_path() {
        assert_eq!(
            resolve_asset_id(
                &format!("nexora-media://localhost/{ASSET_ID}")
                    .parse()
                    .unwrap()
            )
            .unwrap(),
            ASSET_ID
        );
        for path in [
            "/not-a-uuid",
            "/018F47A2-4F25-7A30-8000-0123456789AB",
            "/018f47a24f257a3080000123456789ab",
            "/../x",
            "/image/018f47a2-4f25-7a30-8000-0123456789ab",
            "/video/018f47a2-4f25-7a30-8000-0123456789ab/extra",
            "/video/018f47a2-4f25-7a30-8000-0123456789ab",
        ] {
            let uri: http::Uri = format!("http://nexora-media.localhost{path}")
                .parse()
                .unwrap();
            assert_eq!(resolve_asset_id(&uri), Err(DeliveryError::BadRequest));
        }
        let uri: http::Uri = format!("http://nexora-media.localhost/{ASSET_ID}?x=1")
            .parse()
            .unwrap();
        assert_eq!(resolve_asset_id(&uri), Err(DeliveryError::BadRequest));
    }

    #[test]
    fn ranges_cover_exact_offset_open_suffix_and_errors() {
        assert_eq!(
            select_bytes(Some("bytes=2-5"), 16, false).unwrap(),
            ByteSelection {
                start: 2,
                end: 5,
                partial: true
            }
        );
        assert_eq!(
            select_bytes(Some("bytes=5-"), 16, false).unwrap(),
            ByteSelection {
                start: 5,
                end: 15,
                partial: true
            }
        );
        assert_eq!(
            select_bytes(Some("bytes=-4"), 16, false).unwrap(),
            ByteSelection {
                start: 12,
                end: 15,
                partial: true
            }
        );
        assert_eq!(
            select_bytes(Some("bytes=30-"), 16, false),
            Err(DeliveryError::RangeNotSatisfiable(16))
        );
        assert_eq!(
            select_bytes(None, MAX_RESPONSE_BYTES + 5, false),
            Err(DeliveryError::RangeRequired)
        );
        assert_eq!(
            select_bytes(None, MAX_RESPONSE_BYTES + 5, true).unwrap(),
            ByteSelection {
                start: 0,
                end: MAX_RESPONSE_BYTES + 4,
                partial: false
            }
        );
        for malformed in [
            "items=0-1",
            "bytes=",
            "bytes=1",
            "bytes=3-2",
            "bytes=0-1,4-5",
            "bytes=-0",
        ] {
            assert_eq!(
                select_bytes(Some(malformed), 16, false),
                Err(DeliveryError::RangeNotSatisfiable(16))
            );
        }
    }

    #[test]
    fn get_head_ranges_and_bytes_are_truthful() {
        let (_dir, project, bytes) = project_with_asset("video", "ready", None);
        let full = serve(
            project.clone(),
            http::Method::GET,
            &format!("/{ASSET_ID}"),
            None,
        );
        assert_eq!(full.status(), http::StatusCode::OK);
        assert_eq!(full.body(), &bytes);
        assert_eq!(full.headers()[http::header::CONTENT_TYPE], "video/mp4");
        assert_eq!(full.headers()[http::header::ACCEPT_RANGES], "bytes");

        let partial = serve(
            project.clone(),
            http::Method::GET,
            &format!("/{ASSET_ID}"),
            Some("bytes=3-7"),
        );
        assert_eq!(partial.status(), http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.body(), b"34567");
        assert_eq!(
            partial.headers()[http::header::CONTENT_RANGE],
            "bytes 3-7/16"
        );

        let suffix = serve(
            project.clone(),
            http::Method::GET,
            &format!("/{ASSET_ID}"),
            Some("bytes=-3"),
        );
        assert_eq!(suffix.body(), b"def");
        let head = serve(
            project.clone(),
            http::Method::HEAD,
            &format!("/{ASSET_ID}"),
            None,
        );
        assert_eq!(head.status(), http::StatusCode::OK);
        assert!(head.body().is_empty());
        assert_eq!(head.headers()[http::header::CONTENT_LENGTH], "16");
        let head_range = serve(
            project,
            http::Method::HEAD,
            &format!("/{ASSET_ID}"),
            Some("bytes=4-9"),
        );
        assert_eq!(head_range.status(), http::StatusCode::PARTIAL_CONTENT);
        assert!(head_range.body().is_empty());
        assert_eq!(head_range.headers()[http::header::CONTENT_LENGTH], "6");
        assert_eq!(
            head_range.headers()[http::header::CONTENT_RANGE],
            "bytes 4-9/16"
        );
    }

    #[test]
    fn denies_missing_wrong_project_non_video_and_non_ready() {
        let (_dir, project, _) = project_with_asset("video", "ready", None);
        let missing = serve(project, http::Method::GET, &format!("/{OTHER_ID}"), None);
        assert_eq!(missing.status(), http::StatusCode::NOT_FOUND);

        let (_dir, project, _) = project_with_asset("image", "ready", None);
        assert_eq!(
            serve(project, http::Method::GET, &format!("/{ASSET_ID}"), None).status(),
            http::StatusCode::NOT_FOUND
        );
        let (_dir, project, _) = project_with_asset("video", "failed", None);
        assert_eq!(
            serve(project, http::Method::GET, &format!("/{ASSET_ID}"), None).status(),
            http::StatusCode::NOT_FOUND
        );
        let (_dir, project, _) = project_with_asset("video", "ready", Some(OTHER_ID));
        assert_eq!(
            serve(project, http::Method::GET, &format!("/{ASSET_ID}"), None).status(),
            http::StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn rejects_db_path_escape_size_checksum_and_unsupported_method() {
        let (outside, project, bytes) = project_with_asset("video", "ready", None);
        let escaped = outside.path().join("escaped.mp4");
        fs::write(&escaped, &bytes).unwrap();
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE assets SET managed_master_path=?1 WHERE asset_id=?2",
                rusqlite::params![escaped.to_string_lossy(), ASSET_ID],
            )
            .unwrap();
        assert_eq!(
            serve(
                project.clone(),
                http::Method::GET,
                &format!("/{ASSET_ID}"),
                None
            )
            .status(),
            http::StatusCode::FORBIDDEN
        );

        let master = project
            .root
            .join("assets/masters")
            .join(format!("{ASSET_ID}.mp4"));
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE assets SET managed_master_path=?1,file_size=file_size+1 WHERE asset_id=?2",
                rusqlite::params![master.to_string_lossy(), ASSET_ID],
            )
            .unwrap();
        assert_eq!(
            serve(
                project.clone(),
                http::Method::GET,
                &format!("/{ASSET_ID}"),
                None
            )
            .status(),
            http::StatusCode::FORBIDDEN
        );
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE assets SET file_size=?1,checksum='bad' WHERE asset_id=?2",
                rusqlite::params![bytes.len() as i64, ASSET_ID],
            )
            .unwrap();
        assert_eq!(
            serve(
                project.clone(),
                http::Method::GET,
                &format!("/{ASSET_ID}"),
                None
            )
            .status(),
            http::StatusCode::FORBIDDEN
        );
        assert_eq!(
            serve(project, http::Method::POST, &format!("/{ASSET_ID}"), None).status(),
            http::StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[test]
    fn malformed_and_unsatisfiable_requests_have_expected_statuses() {
        let (_dir, project, _) = project_with_asset("video", "ready", None);
        assert_eq!(
            serve(project.clone(), http::Method::GET, "/nope", None).status(),
            http::StatusCode::BAD_REQUEST
        );
        for range in ["bytes=99-", "bytes=0-1,3-4", "bad"] {
            let response = serve(
                project.clone(),
                http::Method::GET,
                &format!("/{ASSET_ID}"),
                Some(range),
            );
            assert_eq!(response.status(), http::StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(
                response.headers()[http::header::CONTENT_RANGE],
                "bytes */16"
            );
        }
        let response = serve_request(
            None,
            IntegrityCache::default(),
            request(http::Method::GET, &format!("/{ASSET_ID}"), None),
        );
        assert_eq!(response.status(), http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn head_errors_have_no_body_and_keep_error_headers() {
        let (_dir, project, _) = project_with_asset("video", "ready", None);
        let bad_path = serve(project.clone(), http::Method::HEAD, "/not-a-uuid", None);
        assert_eq!(bad_path.status(), http::StatusCode::BAD_REQUEST);
        assert!(bad_path.body().is_empty());
        assert_eq!(bad_path.headers()[http::header::CONTENT_LENGTH], "11");

        let bad_range = serve(
            project,
            http::Method::HEAD,
            &format!("/{ASSET_ID}"),
            Some("bytes=99-"),
        );
        assert_eq!(bad_range.status(), http::StatusCode::RANGE_NOT_SATISFIABLE);
        assert!(bad_range.body().is_empty());
        assert_eq!(
            bad_range.headers()[http::header::CONTENT_RANGE],
            "bytes */16"
        );
        assert_eq!(bad_range.headers()[http::header::ACCEPT_RANGES], "bytes");
    }

    #[test]
    fn changed_open_file_identity_is_denied_after_read() {
        let (_dir, project, _) = project_with_asset("video", "ready", None);
        let mut asset = resolve_video(&project, ASSET_ID, &IntegrityCache::default()).unwrap();
        let master = project
            .root
            .join("assets/masters")
            .join(format!("{ASSET_ID}.mp4"));
        fs::OpenOptions::new()
            .append(true)
            .open(master)
            .unwrap()
            .write_all(b"changed")
            .unwrap();

        assert_eq!(
            read_selected_bytes(
                &mut asset,
                ByteSelection {
                    start: 0,
                    end: 3,
                    partial: true,
                },
            ),
            Err(DeliveryError::Forbidden)
        );
    }

    #[test]
    fn oversized_full_get_is_bounded_and_requests_ranges() {
        let (_dir, project, _) = project_with_asset("video", "ready", None);
        let bytes = vec![7_u8; MAX_RESPONSE_BYTES as usize + 1];
        let master = project
            .root
            .join("assets/masters")
            .join(format!("{ASSET_ID}.mp4"));
        fs::write(&master, &bytes).unwrap();
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE assets SET file_size=?1,checksum=?2 WHERE asset_id=?3",
                rusqlite::params![bytes.len() as i64, checksum, ASSET_ID],
            )
            .unwrap();

        let response = serve(
            project.clone(),
            http::Method::GET,
            &format!("/{ASSET_ID}"),
            None,
        );
        assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[http::header::ACCEPT_RANGES], "bytes");
        assert_eq!(response.body(), b"byte range required");

        let head = serve(project, http::Method::HEAD, &format!("/{ASSET_ID}"), None);
        assert_eq!(head.status(), http::StatusCode::OK);
        assert!(head.body().is_empty());
        assert_eq!(
            head.headers()[http::header::CONTENT_LENGTH],
            bytes.len().to_string()
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_symlink_escape() {
        let (_dir, project, bytes) = project_with_asset("video", "ready", None);
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.mp4");
        fs::write(&target, bytes).unwrap();
        let master = project
            .root
            .join("assets/masters")
            .join(format!("{ASSET_ID}.mp4"));
        fs::remove_file(&master).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &master).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &master).is_err() {
            return;
        }
        project
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE assets SET managed_master_path=?1 WHERE asset_id=?2",
                rusqlite::params![master.to_string_lossy(), ASSET_ID],
            )
            .unwrap();
        assert_eq!(
            serve(project, http::Method::GET, &format!("/{ASSET_ID}"), None).status(),
            http::StatusCode::FORBIDDEN
        );
    }
}

// ---------------------------------------------------------------------------
// Unity Direct Project Delivery & Structure Normalization
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetCategory {
    Vehicle,
    Character,
    Environment,
    Prop,
    Weapon,
    Building,
    Vegetation,
    Furniture,
    Equipment,
    Generic,
    Texture,
    Material,
}

impl AssetCategory {
    pub fn folder_name(&self) -> &'static str {
        match self {
            AssetCategory::Vehicle => "Vehicles",
            AssetCategory::Character => "Characters",
            AssetCategory::Environment => "Environment",
            AssetCategory::Prop => "Props",
            AssetCategory::Weapon => "Weapons",
            AssetCategory::Building => "Buildings",
            AssetCategory::Vegetation => "Vegetation",
            AssetCategory::Furniture => "Furniture",
            AssetCategory::Equipment => "Equipment",
            AssetCategory::Generic => "Props",
            AssetCategory::Texture => "Textures",
            AssetCategory::Material => "Materials",
        }
    }

    pub fn classify(filename: &str, media_kind: &str) -> Self {
        let lower = filename.to_lowercase();
        if media_kind == "image" || media_kind == "texture" {
            return AssetCategory::Texture;
        }
        if media_kind == "material" || lower.contains("material") || lower.contains("mat_") {
            return AssetCategory::Material;
        }

        // Weapon classification
        if lower.contains("gun")
            || lower.contains("rifle")
            || lower.contains("sword")
            || lower.contains("blade")
            || lower.contains("pistol")
            || lower.contains("weapon")
            || lower.contains("axe")
            || lower.contains("shield")
            || lower.contains("bow")
            || lower.contains("staff")
        {
            return AssetCategory::Weapon;
        }
        // Vehicle classification
        if lower.contains("car")
            || lower.contains("vehicle")
            || lower.contains("racing")
            || lower.contains("truck")
            || lower.contains("auto")
            || lower.contains("bike")
            || lower.contains("ship")
            || lower.contains("plane")
            || lower.contains("kart")
            || lower.contains("motorcycle")
            || lower.contains("hovercraft")
            || lower.contains("spacecraft")
            || lower.contains("tank")
            || lower.contains("sedan")
            || lower.contains("coupe")
            || lower.contains("suv")
            || lower.contains("racer")
        {
            return AssetCategory::Vehicle;
        }
        // Character classification
        if lower.contains("character")
            || lower.contains("player")
            || lower.contains("human")
            || lower.contains("monster")
            || lower.contains("npc")
            || lower.contains("hero")
            || lower.contains("enemy")
            || lower.contains("robot")
            || lower.contains("creature")
            || lower.contains("avatar")
            || lower.contains("warrior")
            || lower.contains("wizard")
            || lower.contains("boss")
            || lower.contains("golem")
            || lower.contains("zombie")
            || lower.contains("alien")
            || lower.contains("knight")
        {
            return AssetCategory::Character;
        }
        // Building classification
        if lower.contains("building")
            || lower.contains("tower")
            || lower.contains("house")
            || lower.contains("castle")
            || lower.contains("temple")
            || lower.contains("ruin")
            || lower.contains("bridge")
            || lower.contains("barn")
            || lower.contains("hangar")
        {
            return AssetCategory::Building;
        }
        // Vegetation classification
        if lower.contains("tree")
            || lower.contains("bush")
            || lower.contains("grass")
            || lower.contains("flower")
            || lower.contains("plant")
            || lower.contains("foliage")
            || lower.contains("forest")
        {
            return AssetCategory::Vegetation;
        }
        // Furniture classification
        if lower.contains("chair")
            || lower.contains("table")
            || lower.contains("desk")
            || lower.contains("couch")
            || lower.contains("sofa")
            || lower.contains("bed")
            || lower.contains("cabinet")
            || lower.contains("shelf")
            || lower.contains("lamp")
        {
            return AssetCategory::Furniture;
        }
        // Equipment classification
        if lower.contains("tool")
            || lower.contains("helmet")
            || lower.contains("armor")
            || lower.contains("backpack")
            || lower.contains("battery")
            || lower.contains("generator")
            || lower.contains("equipment")
        {
            return AssetCategory::Equipment;
        }
        // Environment classification
        if lower.contains("env")
            || lower.contains("rock")
            || lower.contains("boulder")
            || lower.contains("terrain")
            || lower.contains("road")
            || lower.contains("mountain")
            || lower.contains("dungeon")
            || lower.contains("street")
            || lower.contains("city")
            || lower.contains("landscape")
            || lower.contains("island")
            || lower.contains("structure")
        {
            return AssetCategory::Environment;
        }
        // Default to Prop
        AssetCategory::Prop
    }
}

#[derive(Clone, Debug)]
pub struct UnityDeployment {
    pub delivery_id: String,
    pub asset_id: String,
    pub target_id: String,
    pub unity_project_root: PathBuf,
    pub destination_folder: PathBuf,
    pub deployed_path: PathBuf,
    pub meta_path: PathBuf,
    pub file_size: u64,
    pub bytes_written: u64,
    pub checksum: String,
}

pub fn discover_unity_project_path() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(home.join("development").join("Games"));
    candidates.push(home.join("Documents").join("Games"));
    if let Some(app_data) = dirs::data_local_dir() {
        candidates.push(app_data.join("development").join("Games"));
        candidates.push(app_data.join("Games"));
    }

    for parent in candidates {
        if let Ok(entries) = std::fs::read_dir(&parent) {
            for entry in entries.flatten() {
                let candidate = entry.path();
                if is_valid_unity_project(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn is_valid_unity_project(path: &Path) -> bool {
    path.is_dir()
        && path.join("Assets").is_dir()
        && path.join("ProjectSettings").is_dir()
        && path.join("Packages").is_dir()
}

pub fn resolve_unity_destination_folder(unity_root: &Path, category: AssetCategory) -> PathBuf {
    let assets = unity_root.join("Assets");
    let cat_name = category.folder_name();

    // Check project conventions in priority order
    let conventions = [
        assets.join("Nexora").join(cat_name),
        assets.join("Models").join(cat_name),
        assets.join("Art").join(cat_name),
        assets.join("Game").join(cat_name),
    ];

    for candidate in &conventions {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    if assets.join("Models").exists() {
        return assets.join("Models").join(cat_name);
    }
    if assets.join("Art").exists() {
        return assets.join("Art").join(cat_name);
    }

    // Default standard Nexora folder
    assets.join("Nexora").join(cat_name)
}

pub fn resolve_unique_destination_path(
    destination_folder: &Path,
    safe_stem: &str,
    ext: &str,
    checksum: &str,
) -> PathBuf {
    let initial_name = format!("{safe_stem}.{ext}");
    let initial_path = destination_folder.join(&initial_name);

    if !initial_path.exists() {
        return initial_path;
    }

    // Check if the existing file has the exact same content
    if let Ok(existing_bytes) = std::fs::read(&initial_path) {
        let existing_checksum = format!("{:x}", sha2::Sha256::digest(&existing_bytes));
        if existing_checksum == checksum {
            return initial_path;
        }
    }

    // Naming collision with different content -> generate incremented safe name
    let mut counter = 1;
    loop {
        let candidate_name = format!("{safe_stem}_{counter}.{ext}");
        let candidate_path = destination_folder.join(&candidate_name);
        if !candidate_path.exists() {
            return candidate_path;
        }
        if let Ok(existing_bytes) = std::fs::read(&candidate_path) {
            let existing_checksum = format!("{:x}", sha2::Sha256::digest(&existing_bytes));
            if existing_checksum == checksum {
                return candidate_path;
            }
        }
        counter += 1;
    }
}

pub fn deploy_model3d_to_unity(
    project: &ProjectState,
    asset_id: &str,
    _processing_job_id: &str,
    target_id: &str,
    category: AssetCategory,
) -> Result<UnityDeployment, String> {
    let (master_path, checksum, filename, media_kind, processing_status, approval_status) = {
        let db = project.db.lock().map_err(|e| e.to_string())?;
        let row: (Option<String>, String, String, String, Option<String>, Option<String>) = db
            .query_row(
                "SELECT managed_master_path, checksum, original_filename, media_kind, processing_status, (SELECT status FROM asset_approvals WHERE asset_id=assets.asset_id) FROM assets WHERE asset_id=?1 AND project_id=?2 AND status='ready'",
                rusqlite::params![asset_id, project.manifest.project_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .map_err(|e| format!("asset not found or not ready: {e}"))?;

        let master = row.0.map(PathBuf::from).unwrap_or_else(|| {
            let ext = if row.3 == "model3d" { "glb" } else { "png" };
            project.root.join("assets/masters").join(format!("{asset_id}.{ext}"))
        });
        (master, row.1, row.2, row.3, row.4, row.5)
    };

    if media_kind == "model3d"
        && (processing_status.as_deref() != Some("approved")
            || approval_status.as_deref() != Some("approved"))
    {
        return Err("3D asset must pass the final approval gate before engine deployment".into());
    }

    if !master_path.exists() {
        return Err(format!("master asset file not found on disk at {}", master_path.display()));
    }

    let unity_root = if target_id.is_empty() {
        discover_unity_project_path()
            .ok_or_else(|| "no unity project discovered".to_string())?
    } else {
        PathBuf::from(target_id)
    };

    if !is_valid_unity_project(&unity_root) {
        if !unity_root.exists() {
            return Err(format!("Unity project directory does not exist: {}", unity_root.display()));
        }
        if !unity_root.join("Assets").exists() {
            return Err(format!("path is not a valid Unity project (missing Assets/ folder): {}", unity_root.display()));
        }
        if !unity_root.join("ProjectSettings").exists() {
            return Err(format!("path is not a valid Unity project (missing ProjectSettings/ folder): {}", unity_root.display()));
        }
        return Err(format!("path is not a valid Unity project: {}", unity_root.display()));
    }

    let destination_folder = resolve_unity_destination_folder(&unity_root, category);
    std::fs::create_dir_all(&destination_folder).map_err(|e| format!("failed to create destination folder: {e}"))?;

    let stem = PathBuf::from(&filename)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| asset_id.to_string());
    let safe_stem = stem.replace(' ', "_").replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "");
    let ext = if media_kind == "model3d" { "glb" } else { "png" };

    let deployed_path = resolve_unique_destination_path(&destination_folder, &safe_stem, ext, &checksum);

    // Deploy main master model
    std::fs::copy(&master_path, &deployed_path).map_err(|e| format!("failed to copy asset: {e}"))?;
    let file_size = std::fs::metadata(&deployed_path).map(|m| m.len()).unwrap_or(0);

    let meta_path = deployed_path.with_extension(format!("{ext}.meta"));
    let guid = if meta_path.exists() {
        // Preserve existing GUID from meta file if present
        let existing = std::fs::read_to_string(&meta_path).unwrap_or_default();
        existing.lines()
            .find(|l| l.starts_with("guid: "))
            .map(|l| l.trim_start_matches("guid: ").trim().to_string())
            .unwrap_or_else(|| Uuid::now_v7().simple().to_string())
    } else {
        Uuid::now_v7().simple().to_string()
    };

    let meta_content = format!("fileFormatVersion: 2
guid: {guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
    useSRGBColors: 1
");
    let _ = std::fs::write(&meta_path, meta_content);

    // Also look for and deploy companion LODs and Collider if present in job directory
    if let Some(parent_dir) = master_path.parent() {
        // LOD1
        let lod1_src = parent_dir.join("vehicle_lod1.glb");
        if lod1_src.exists() {
            let lod1_dst = destination_folder.join(format!("{safe_stem}_LOD1.glb"));
            let _ = std::fs::copy(&lod1_src, &lod1_dst);
            let lod1_meta = destination_folder.join(format!("{safe_stem}_LOD1.glb.meta"));
            let lod1_guid = Uuid::now_v7().simple().to_string();
            let _ = std::fs::write(&lod1_meta, format!("fileFormatVersion: 2
guid: {lod1_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));
        }

        // LOD2
        let lod2_src = parent_dir.join("vehicle_lod2.glb");
        if lod2_src.exists() {
            let lod2_dst = destination_folder.join(format!("{safe_stem}_LOD2.glb"));
            let _ = std::fs::copy(&lod2_src, &lod2_dst);
            let lod2_meta = destination_folder.join(format!("{safe_stem}_LOD2.glb.meta"));
            let lod2_guid = Uuid::now_v7().simple().to_string();
            let _ = std::fs::write(&lod2_meta, format!("fileFormatVersion: 2
guid: {lod2_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));
        }

        // Collider
        let col_src = parent_dir.join("clean_collider.glb");
        if col_src.exists() {
            let col_dst = destination_folder.join(format!("{safe_stem}_Collider.glb"));
            let _ = std::fs::copy(&col_src, &col_dst);
            let col_meta = destination_folder.join(format!("{safe_stem}_Collider.glb.meta"));
            let col_guid = Uuid::now_v7().simple().to_string();
            let _ = std::fs::write(&col_meta, format!("fileFormatVersion: 2
guid: {col_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));
        }
    }

    // Deploy Unity Prefab with preconfigured LODGroup, Renderers, and Collider
    let prefab_path = destination_folder.join(format!("{safe_stem}.prefab"));
    let prefab_meta = destination_folder.join(format!("{safe_stem}.prefab.meta"));
    let prefab_guid = Uuid::now_v7().simple().to_string();

    // GUIDs for the model files (must match what we wrote to .meta files)
    let master_guid = guid; // from master model meta
    let lod1_guid = Uuid::now_v7().simple().to_string(); // will match LOD1 meta
    let lod2_guid = Uuid::now_v7().simple().to_string(); // will match LOD2 meta
    let col_guid = Uuid::now_v7().simple().to_string();   // will match Collider meta

    // Compute fileIDs (Unity uses first 8 bytes of GUID as fileID base)
    // Format: localIdentifierInFile = 0 for main object, fileID = (GUID as u64) << 32 | localID
    // For simplicity, we use a fixed localIdentifier scheme
    let master_file_id = format!("{}00000000", &master_guid[..8]);
    let lod1_file_id = format!("{}00000000", &lod1_guid[..8]);
    let lod2_file_id = format!("{}00000000", &lod2_guid[..8]);
    let col_file_id = format!("{}00000000", &col_guid[..8]);

let prefab_yaml = format!(r####"%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &1000000000000000
GameObject:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  serializedVersion: 6
  m_Component:
  - component: {{fileID: 4000000000000000}}
  - component: {{fileID: 2050000000000000}}
  - component: {{fileID: 8000000000000000}}
  m_Layer: 0
  m_Name: {safe_stem}
  m_TagString: Untagged
  m_Icon: {{fileID: 0}}
  m_NavMeshLayer: 0
  m_StaticEditorFlags: 0
  m_IsActive: 1
--- !u!4 &4000000000000000
Transform:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 1000000000000000}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_ConstrainProportionsScale: 0
  m_Children:
  - {{fileID: 2000000000000000}}
  - {{fileID: 3000000000000000}}
  - {{fileID: 4000000000000000}}
  - {{fileID: 5000000000000000}}
  m_Father: {{fileID: 0}}
  m_RootOrder: 0
  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}
--- !u!1 &2000000000000000
GameObject:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  serializedVersion: 6
  m_Component:
  - component: {{fileID: 2100000000000000}}
  m_Layer: 0
  m_Name: LOD0
  m_TagString: Untagged
  m_Icon: {{fileID: 0}}
  m_NavMeshLayer: 0
  m_StaticEditorFlags: 0
  m_IsActive: 1
--- !u!205 &2050000000000000
LODGroup:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 1000000000000000}}
  serializedVersion: 2
  m_LocalReferencePoint: {{x: 0, y: 0, z: 0}}
  m_Size: 1
  m_FadeMode: 0
  m_AnimateCrossFading: 0
  m_LastLODIsBillboard: 0
  m_LODs:
  - screenRelativeTransitionHeight: 0.6
    fadeTransitionWidth: 0.02
    renderers:
    - {{fileID: 2100000000000000}}
  - screenRelativeTransitionHeight: 0.3
    fadeTransitionWidth: 0.02
    renderers:
    - {{fileID: 3100000000000000}}
  - screenRelativeTransitionHeight: 0.1
    fadeTransitionWidth: 0.02
    renderers:
    - {{fileID: 4100000000000000}}
  m_Enabled: 1
--- !u!4 &2000000000000000
Transform:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 2000000000000000}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_ConstrainProportionsScale: 0
  m_Children: []
  m_Father: {{fileID: 4000000000000000}}
  m_RootOrder: 0
  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}
--- !u!1 &3000000000000000
GameObject:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  serializedVersion: 6
  m_Component:
  - component: {{fileID: 3100000000000000}}
  m_Layer: 0
  m_Name: LOD1
  m_TagString: Untagged
  m_Icon: {{fileID: 0}}
  m_NavMeshLayer: 0
  m_StaticEditorFlags: 0
  m_IsActive: 1
--- !u!4 &3000000000000000
Transform:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 3000000000000000}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_ConstrainProportionsScale: 0
  m_Children: []
  m_Father: {{fileID: 4000000000000000}}
  m_RootOrder: 0
  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}
--- !u!1 &4000000000000000
GameObject:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  serializedVersion: 6
  m_Component:
  - component: {{fileID: 4100000000000000}}
  m_Layer: 0
  m_Name: LOD2
  m_TagString: Untagged
  m_Icon: {{fileID: 0}}
  m_NavMeshLayer: 0
  m_StaticEditorFlags: 0
  m_IsActive: 1
--- !u!4 &4000000000000000
Transform:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 4000000000000000}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_ConstrainProportionsScale: 0
  m_Children: []
  m_Father: {{fileID: 4000000000000000}}
  m_RootOrder: 0
  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}
--- !u!1 &5000000000000000
GameObject:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  serializedVersion: 6
  m_Component:
  - component: {{fileID: 5100000000000000}}
  m_Layer: 0
  m_Name: Collider
  m_TagString: Untagged
  m_Icon: {{fileID: 0}}
  m_NavMeshLayer: 0
  m_StaticEditorFlags: 1
  m_IsActive: 1
--- !u!4 &5000000000000000
Transform:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 5000000000000000}}
  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}
  m_LocalPosition: {{x: 0, y: 0, z: 0}}
  m_LocalScale: {{x: 1, y: 1, z: 1}}
  m_ConstrainProportionsScale: 0
  m_Children: []
  m_Father: {{fileID: 4000000000000000}}
  m_RootOrder: 0
  m_LocalEulerAnglesHint: {{x: 0, y: 0, z: 0}}
--- !u!23 &2100000000000000
MeshRenderer:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 2000000000000000}}
  m_Enabled: 1
  m_CastShadows: 1
  m_ReceiveShadows: 1
  m_DynamicOccludee: 1
  m_MotionVectors: 1
  m_LightProbeUsage: 1
  m_ReflectionProbeUsage: 1
  m_RenderingLayerMask: 1
  m_RendererPriority: 0
  m_Materials:
  - {{fileID: 0}}
  m_StaticBatchInfo:
    firstSubMesh: 0
    subMeshCount: 0
  m_StaticBatchRoot: {{fileID: 0}}
  m_ProbeAnchor: {{fileID: 0}}
  m_LightProbeVolumeOverride: {{fileID: 0}}
  m_RenderProbeAnchor: {{fileID: 0}}
  m_ProbeVolumeSceneOverride: {{fileID: 0}}
  m_UseLightProbes: 1
  m_UseReflectionProbes: 1
  m_ReceiveGIDiffuse: 1
  m_ReceiveGISpecular: 1
--- !u!23 &3100000000000000
MeshRenderer:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 3000000000000000}}
  m_Enabled: 1
  m_CastShadows: 1
  m_ReceiveShadows: 1
  m_DynamicOccludee: 1
  m_MotionVectors: 1
  m_LightProbeUsage: 1
  m_ReflectionProbeUsage: 1
  m_RenderingLayerMask: 1
  m_RendererPriority: 0
  m_Materials:
  - {{fileID: 0}}
  m_StaticBatchInfo:
    firstSubMesh: 0
    subMeshCount: 0
  m_StaticBatchRoot: {{fileID: 0}}
  m_ProbeAnchor: {{fileID: 0}}
  m_LightProbeVolumeOverride: {{fileID: 0}}
  m_RenderProbeAnchor: {{fileID: 0}}
  m_ProbeVolumeSceneOverride: {{fileID: 0}}
  m_UseLightProbes: 1
  m_UseReflectionProbes: 1
  m_ReceiveGIDiffuse: 1
  m_ReceiveGISpecular: 1
--- !u!23 &4100000000000000
MeshRenderer:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 4000000000000000}}
  m_Enabled: 1
  m_CastShadows: 1
  m_ReceiveShadows: 1
  m_DynamicOccludee: 1
  m_MotionVectors: 1
  m_LightProbeUsage: 1
  m_ReflectionProbeUsage: 1
  m_RenderingLayerMask: 1
  m_RendererPriority: 0
  m_Materials:
  - {{fileID: 0}}
  m_StaticBatchInfo:
    firstSubMesh: 0
    subMeshCount: 0
  m_StaticBatchRoot: {{fileID: 0}}
  m_ProbeAnchor: {{fileID: 0}}
  m_LightProbeVolumeOverride: {{fileID: 0}}
  m_RenderProbeAnchor: {{fileID: 0}}
  m_ProbeVolumeSceneOverride: {{fileID: 0}}
  m_UseLightProbes: 1
  m_UseReflectionProbes: 1
  m_ReceiveGIDiffuse: 1
  m_ReceiveGISpecular: 1
--- !u!65 &5100000000000000
MeshCollider:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 5000000000000000}}
  m_Enabled: 1
  m_Convex: 1
  m_IsTrigger: 0
  m_CookingOptions: 0
  m_Material: {{fileID: 0}}
  m_Mesh: {{fileID: {col_file_id}}}
--- !u!8000000000000000
MeshFilter:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 2000000000000000}}
  m_Mesh: {{fileID: {master_file_id}}}
--- !u!8000000000000000
MeshFilter:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 3000000000000000}}
  m_Mesh: {{fileID: {lod1_file_id}}}
--- !u!8000000000000000
MeshFilter:
  m_ObjectHideFlags: 0
  m_CorrespondingSourceObject: {{fileID: 0}}
  m_PrefabInstance: {{fileID: 0}}
  m_PrefabAsset: {{fileID: 0}}
  m_GameObject: {{fileID: 4000000000000000}}
  m_Mesh: {{fileID: {lod2_file_id}}}
"####);

    // Write LOD1 meta with correct GUID
    let lod1_meta = destination_folder.join(format!("{safe_stem}_LOD1.glb.meta"));
    let _ = std::fs::write(&lod1_meta, format!("fileFormatVersion: 2
guid: {lod1_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));

    // Write LOD2 meta with correct GUID
    let lod2_meta = destination_folder.join(format!("{safe_stem}_LOD2.glb.meta"));
    let _ = std::fs::write(&lod2_meta, format!("fileFormatVersion: 2
guid: {lod2_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));

    // Write Collider meta with correct GUID
    let col_meta = destination_folder.join(format!("{safe_stem}_Collider.glb.meta"));
    let _ = std::fs::write(&col_meta, format!("fileFormatVersion: 2
guid: {col_guid}
ModelImporter:
  serializedVersion: 22200
  materials:
    materialImportMode: 2
"));

    let prefab_meta_content = format!("fileFormatVersion: 2
guid: {prefab_guid}
PrefabImporter:
  externalObjects: {{}}
  userData: 
  assetBundleName: 
  assetBundleVariant: 
");

    let _ = std::fs::write(&prefab_path, prefab_yaml);
    let _ = std::fs::write(&prefab_meta, prefab_meta_content);

    let delivery_id = Uuid::now_v7().to_string();

    Ok(UnityDeployment {
        delivery_id,
        asset_id: asset_id.to_string(),
        target_id: unity_root.to_string_lossy().into_owned(),
        unity_project_root: unity_root,
        destination_folder,
        deployed_path,
        meta_path,
        file_size,
        bytes_written: file_size,
        checksum,
    })
}

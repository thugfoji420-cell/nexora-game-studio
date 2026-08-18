use crate::{AppState, project::ProjectState};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, Metadata},
    hash::{Hash, Hasher},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
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

//! Overnight batch generation queue.
//!
//! A batch is an ordered list of text prompts executed sequentially at a fixed
//! quality, each through the normal Hunyuan job pipeline (VRAM preflight,
//! worker execution, Blender processing). The manifest is persisted under
//! `<project>/batch-queue/<batch_id>.json` after every transition, so a crash
//! or app restart never loses progress: on the next UI poll the manifest is
//! reloaded and marked paused with one click to resume from the cursor.
//!
//! Pause and cancel take effect at item boundaries — the in-flight generation
//! always finishes and is recorded.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::hardware::HardwareSnapshot;
use crate::hunyuan_generation::{HunyuanGenerationRequest, HunyuanMode, HUNYUAN_PROFILE_ID};
use crate::jobs::{self, CreateHunyuanGenerationJobInput, JobError};
use crate::project::ProjectState;
use crate::providers::ProviderRegistry;

pub const BATCH_DIR_NAME: &str = "batch-queue";
const ITEM_TIMEOUT_SECS: u64 = 120 * 60;
const POLL_INTERVAL_MS: u64 = 2_000;
const DEFAULT_NEGATIVE_PROMPT: &str =
    "blurry, ugly, deformed, low quality, artifacts, distorted, noisy, watermark";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Running,
    Paused,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BatchItemStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub prompt: String,
    pub status: BatchItemStatus,
    pub job_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchManifest {
    pub schema_version: u32,
    pub batch_id: String,
    pub quality: String,
    pub items: Vec<BatchItem>,
    pub status: BatchStatus,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub note: Option<String>,
}

/// Shared handle for the active batch: the runner thread and the Tauri
/// commands both operate on this. One active batch at a time.
pub type ActiveBatch = Arc<Mutex<Option<Arc<Mutex<BatchManifest>>>>>;

static RUNNING_BATCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn batch_dir(project: &ProjectState) -> PathBuf {
    project.root().join(BATCH_DIR_NAME)
}

fn manifest_path(project: &ProjectState, batch_id: &str) -> PathBuf {
    batch_dir(project).join(format!("{batch_id}.json"))
}

fn load_manifest(project: &ProjectState, batch_id: &str) -> Option<BatchManifest> {
    let text = std::fs::read_to_string(manifest_path(project, batch_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Newest paused manifest for the current project, if any. Called lazily by
/// the UI state command so a batch paused by an app restart resurfaces.
pub fn load_latest_paused(project: &ProjectState) -> Option<BatchManifest> {
    let dir = batch_dir(project);
    let mut best: Option<BatchManifest> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<BatchManifest>(&text) else {
            continue;
        };
        if manifest.status == BatchStatus::Paused {
            let replace = best
                .as_ref()
                .map(|b| manifest.updated_at_ms > b.updated_at_ms)
                .unwrap_or(true);
            if replace {
                best = Some(manifest);
            }
        }
    }
    best
}

fn persist(project: &ProjectState, manifest: &BatchManifest) -> Result<(), String> {
    let dir = batch_dir(project);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = manifest_path(project, &manifest.batch_id);
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// On app start / project open: any manifest still marked `running` belongs to
/// a runner thread that died with the previous process. Mark it paused so the
/// UI offers one-click resume from the cursor.
pub fn recover_interrupted(project: &ProjectState) -> Vec<String> {
    let dir = batch_dir(project);
    let mut recovered = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return recovered,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut manifest) = serde_json::from_str::<BatchManifest>(&text) else {
            continue;
        };
        if manifest.status == BatchStatus::Running {
            manifest.status = BatchStatus::Paused;
            manifest.note = Some("App restarted while the batch was running — resume to continue.".into());
            manifest.updated_at_ms = now_ms();
            for item in &mut manifest.items {
                if item.status == BatchItemStatus::Running {
                    item.status = BatchItemStatus::Pending;
                }
            }
            if persist(project, &manifest).is_ok() {
                recovered.push(manifest.batch_id.clone());
            }
        }
    }
    recovered
}

fn validate_prompts(prompts: &[String]) -> Result<(), String> {
    if prompts.is_empty() {
        return Err("batch needs at least one prompt".into());
    }
    if prompts.len() > 50 {
        return Err("batch is limited to 50 prompts".into());
    }
    if prompts.iter().any(|p| p.trim().is_empty()) {
        return Err("batch prompts must not be empty".into());
    }
    Ok(())
}

/// Start a new batch: persist the manifest and spawn the runner thread.
#[allow(clippy::too_many_arguments)]
pub fn start_batch(
    project: Arc<ProjectState>,
    registry: Arc<Mutex<ProviderRegistry>>,
    snapshot: HardwareSnapshot,
    prompts: Vec<String>,
    quality: String,
) -> Result<Arc<Mutex<BatchManifest>>, String> {
    validate_prompts(&prompts)?;
    let quality = quality.trim().to_lowercase();
    if !matches!(quality.as_str(), "draft" | "standard" | "high") {
        return Err("quality must be draft, standard or high".into());
    }
    if RUNNING_BATCH.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("a batch is already running".into());
    }

    let batch_id = uuid::Uuid::now_v7().to_string();
    let manifest = BatchManifest {
        schema_version: 1,
        batch_id: batch_id.clone(),
        quality,
        items: prompts
            .iter()
            .map(|prompt| BatchItem {
                prompt: prompt.trim().to_string(),
                status: BatchItemStatus::Pending,
                job_id: None,
                error: None,
            })
            .collect(),
        status: BatchStatus::Running,
        created_at_ms: now_ms(),
        updated_at_ms: now_ms(),
        note: None,
    };
    persist(&project, &manifest)?;

    let shared = Arc::new(Mutex::new(manifest));
    if let Err(error) = spawn_runner(project.clone(), registry, snapshot, shared.clone()) {
        // Avoid leaving a ghost Running manifest on disk.
        let mut m = shared.lock().unwrap();
        m.status = BatchStatus::Cancelled;
        m.note = Some(error.clone());
        let _ = persist(&project, &m);
        return Err(error);
    }
    Ok(shared)
}

/// (Re)spawn the runner thread for an existing manifest. Used by start and
/// by the resume command after a pause or an app restart.
pub fn spawn_runner(
    project: Arc<ProjectState>,
    registry: Arc<Mutex<ProviderRegistry>>,
    snapshot: HardwareSnapshot,
    shared: Arc<Mutex<BatchManifest>>,
) -> Result<(), String> {
    if RUNNING_BATCH.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("a batch is already running".into());
    }
    {
        let mut m = shared.lock().unwrap();
        m.status = BatchStatus::Running;
        m.note = None;
        m.updated_at_ms = now_ms();
    }
    if persist(&project, &shared.lock().unwrap()).is_err() {
        RUNNING_BATCH.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err("failed to persist batch manifest".into());
    }
    let runner_manifest = shared.clone();
    let spawn_result = std::thread::Builder::new()
        .name("batch-queue-runner".into())
        .spawn(move || {
            let _gpu = crate::gpu_guard::GenerationGuard::acquire();
            run_loop(&project, &registry, &snapshot, &runner_manifest);
            RUNNING_BATCH.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    if spawn_result.is_err() {
        RUNNING_BATCH.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err("failed to spawn batch runner thread".into());
    }
    Ok(())
}

/// Pause at the next item boundary (an in-flight generation finishes and is
/// recorded before the runner exits).
pub fn pause_batch(project: &ProjectState, shared: &Arc<Mutex<BatchManifest>>) -> Result<(), String> {
    let mut m = shared.lock().unwrap();
    if m.status != BatchStatus::Running {
        return Err("batch is not running".into());
    }
    m.status = BatchStatus::Paused;
    m.note = Some("Paused — resume to continue from here.".into());
    m.updated_at_ms = now_ms();
    let snapshot = m.clone();
    drop(m);
    persist(project, &snapshot)
}

/// Cancel at the next item boundary; completed items are kept.
pub fn cancel_batch(project: &ProjectState, shared: &Arc<Mutex<BatchManifest>>) -> Result<(), String> {
    let mut m = shared.lock().unwrap();
    m.status = BatchStatus::Cancelled;
    m.note = Some("Cancelled — completed items are kept.".into());
    m.updated_at_ms = now_ms();
    let snapshot = m.clone();
    drop(m);
    persist(project, &snapshot)
}

/// Resume a paused batch (also used after an app restart). Holds the GPU
/// guard for the remainder of the batch so A1111 stays parked.
pub fn resume_batch(
    project: Arc<ProjectState>,
    registry: Arc<Mutex<ProviderRegistry>>,
    snapshot: HardwareSnapshot,
    shared: &Arc<Mutex<BatchManifest>>,
) -> Result<(), String> {
    if RUNNING_BATCH.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("a batch is already running".into());
    }
    spawn_runner(project, registry, snapshot, shared.clone())
}

fn run_loop(
    project: &Arc<ProjectState>,
    registry: &Arc<Mutex<ProviderRegistry>>,
    snapshot: &HardwareSnapshot,
    manifest: &Arc<Mutex<BatchManifest>>,
) {
    loop {
        let action = {
            let mut m = manifest.lock().unwrap();
            if m.status != BatchStatus::Running {
                break;
            }
            match m.items.iter().position(|item| item.status == BatchItemStatus::Pending) {
                Some(index) => {
                    m.items[index].status = BatchItemStatus::Running;
                    m.updated_at_ms = now_ms();
                    let snapshot_manifest = m.clone();
                    (index, m.items[index].prompt.clone(), m.quality.clone(), snapshot_manifest)
                }
                None => {
                    m.status = BatchStatus::Completed;
                    m.updated_at_ms = now_ms();
                    break;
                }
            }
        };
        let (index, prompt, quality, snapshot_manifest) = action;
        // Persist the Running mark immediately: if the app crashes mid-item,
        // recovery must not find an item permanently stuck in Running.
        if persist(project, &snapshot_manifest).is_err() {
            break;
        }
        let outcome = run_item(project, registry, snapshot, &prompt, &quality);
        let mut m = manifest.lock().unwrap();
        match outcome {
            Ok(job_id) => {
                m.items[index].status = BatchItemStatus::Completed;
                m.items[index].job_id = Some(job_id);
            }
            Err(error) => {
                m.items[index].status = BatchItemStatus::Failed;
                m.items[index].error = Some(error);
            }
        }
        m.updated_at_ms = now_ms();
        let snapshot_manifest = m.clone();
        drop(m);
        if persist(project, &snapshot_manifest).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2_000));
    }
}

fn run_item(
    project: &Arc<ProjectState>,
    registry: &Arc<Mutex<ProviderRegistry>>,
    snapshot: &HardwareSnapshot,
    prompt: &str,
    quality: &str,
) -> Result<String, String> {
    let input = CreateHunyuanGenerationJobInput {
        request: HunyuanGenerationRequest {
            schema_version: 1,
            mode: HunyuanMode::TextTo3d,
            prompt: prompt.to_string(),
            negative_prompt: Some(DEFAULT_NEGATIVE_PROMPT.into()),
            source_asset_id: None,
            profile: HUNYUAN_PROFILE_ID.to_string(),
            quality: quality.to_string(),
            seed: None,
            output_format: "glb".into(),
            generate_texture: Some(true),
        },
        provider_id: None,
    };
    let created = {
        let registry = registry.lock().unwrap();
        jobs::create_hunyuan_generation(project, &registry, snapshot, input)
            .map_err(|error| match error {
                JobError::InvalidInput(message) => format!("blocked: {message}"),
                other => other.to_string(),
            })?
    };
    let job_id = created.job.job_id.clone();

    let deadline = std::time::Instant::now() + Duration::from_secs(ITEM_TIMEOUT_SECS);
    loop {
        std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        let status = {
            let db = project.db.lock().unwrap();
            db.query_row(
                "SELECT status, error_message FROM jobs WHERE job_id = ?1",
                [&job_id],
                |row| {
                    let status: String = row.get(0)?;
                    let error: Option<String> = row.get(1)?;
                    Ok((status, error))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
        };
        match status {
            Some((status, error)) => match status.as_str() {
                "completed" => return Ok(job_id),
                "failed" => {
                    return Err(error.unwrap_or_else(|| "generation failed".into()));
                }
                "cancelled" => return Err("generation cancelled".into()),
                _ => {}
            },
            None => return Err("job row disappeared".into()),
        }
        if std::time::Instant::now() > deadline {
            return Err("generation timed out".into());
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers / pure logic
// ---------------------------------------------------------------------------

pub fn next_pending_index(items: &[BatchItem]) -> Option<usize> {
    items.iter().position(|item| item.status == BatchItemStatus::Pending)
}

pub fn summarize(manifest: &BatchManifest) -> (usize, usize, usize, usize) {
    let mut completed = 0;
    let mut failed = 0;
    let mut pending = 0;
    for item in &manifest.items {
        match item.status {
            BatchItemStatus::Completed => completed += 1,
            BatchItemStatus::Failed | BatchItemStatus::Blocked => failed += 1,
            _ => pending += 1,
        }
    }
    (completed, failed, pending, manifest.items.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(status: BatchItemStatus) -> BatchItem {
        BatchItem {
            prompt: "car".into(),
            status,
            job_id: None,
            error: None,
        }
    }

    #[test]
    fn next_pending_skips_finished_items() {
        let items = vec![
            item(BatchItemStatus::Completed),
            item(BatchItemStatus::Failed),
            item(BatchItemStatus::Pending),
            item(BatchItemStatus::Pending),
        ];
        assert_eq!(next_pending_index(&items), Some(2));
        assert_eq!(
            next_pending_index(&[item(BatchItemStatus::Completed)]),
            None
        );
    }

    #[test]
    fn summarize_counts_buckets() {
        let manifest = BatchManifest {
            schema_version: 1,
            batch_id: "b".into(),
            quality: "high".into(),
            items: vec![
                item(BatchItemStatus::Completed),
                item(BatchItemStatus::Failed),
                item(BatchItemStatus::Blocked),
                item(BatchItemStatus::Pending),
                item(BatchItemStatus::Running),
            ],
            status: BatchStatus::Running,
            created_at_ms: 0,
            updated_at_ms: 0,
            note: None,
        };
        assert_eq!(summarize(&manifest), (1, 2, 2, 5));
    }

    #[test]
    fn validation_rejects_empty_and_oversized() {
        assert!(validate_prompts(&[]).is_err());
        assert!(validate_prompts(&["  ".to_string()]).is_err());
        let too_many: Vec<String> = (0..51).map(|i| format!("car {i}")).collect();
        assert!(validate_prompts(&too_many).is_err());
        assert!(validate_prompts(&["one car".into(), "two cars".into()]).is_ok());
    }

    #[test]
    fn manifest_serializes_camel_case() {
        let manifest = BatchManifest {
            schema_version: 1,
            batch_id: "id-1".into(),
            quality: "high".into(),
            items: vec![item(BatchItemStatus::Pending)],
            status: BatchStatus::Paused,
            created_at_ms: 1,
            updated_at_ms: 2,
            note: Some("hello".into()),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("\"batchId\""));
        assert!(json.contains("\"updatedAtMs\""));
        assert!(json.contains("\"snake_case\"") || json.contains("\"paused\""));
    }
}

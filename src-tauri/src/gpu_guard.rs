//! GPU arbitration between the Hunyuan3D server and A1111 on small-VRAM GPUs.
//!
//! On the reference 4 GB card, any Hunyuan generation dies with CUDA OOM while
//! A1111 holds ~2.5 GB. This module parks A1111 while generations run and
//! brings it back when the last generation finishes, so the two services stop
//! fighting over the GPU manually.
//!
//! Usage: `GenerationGuard::acquire()` at the top of a generation execution;
//! drop handles the release. Guards nest (batch queues hold one guard for the
//! whole batch), and A1111 is only relaunched when the last guard drops — and
//! only if A1111 was actually running when the first guard was acquired.

use std::net::TcpStream;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::runtime_manager::RuntimeManager;

const A1111_RUNTIME_ID: &str = "automatic1111";
const A1111_PORT: u16 = 7860;
const MIN_RESTART_GAP_MS: u128 = 90_000;

struct GpuGuardContext {
    runtime_manager: Arc<RuntimeManager>,
}

static CTX: OnceLock<GpuGuardContext> = OnceLock::new();
static ACTIVE_GENERATIONS: AtomicU32 = AtomicU32::new(0);
static A1111_WAS_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_A1111_RELAUNCH_MS: Mutex<u128> = Mutex::new(0);

/// Install the guard context once at app startup. Safe to call again; later
/// calls are ignored.
pub fn install(runtime_manager: Arc<RuntimeManager>) {
    let _ = CTX.set(GpuGuardContext { runtime_manager });
}

fn a1111_listening() -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{A1111_PORT}").parse().unwrap(),
        Duration::from_millis(400),
    )
    .is_ok()
}

/// Find and force-kill whichever process is listening on the A1111 port.
/// Used when A1111 was started outside Nexora (so the runtime manager holds
/// no child handle for it).
fn kill_port_owner(port: u16) -> Option<u32> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5
            && fields[1].ends_with(&format!(":{port}"))
            && fields[3] == "LISTENING"
            && let Ok(pid) = fields[4].parse::<u32>()
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
            return Some(pid);
        }
    }
    None
}

/// Park A1111: suppress the watchdog for it, then stop it whether Nexora
/// spawned it or it was running externally.
fn park_a1111(ctx: &GpuGuardContext) {
    let manager = ctx.runtime_manager.clone();
    tauri::async_runtime::block_on(async move {
        manager.suppress_runtime(A1111_RUNTIME_ID, true).await;
        // Clears the runtime state so the watchdog cannot mistake a stale
        // Ready flag for a live process.
        let _ = manager.stop_runtime(A1111_RUNTIME_ID).await;
    });
    if A1111_WAS_RUNNING.load(Ordering::SeqCst) && a1111_listening() {
        if let Some(pid) = kill_port_owner(A1111_PORT) {
            println!("[gpu-guard] parked external A1111 (pid {pid}) for generation");
        }
    }
}

fn relaunch_a1111_if_needed(ctx: &GpuGuardContext) {
    manager_suppress_clear(ctx);
    if !A1111_WAS_RUNNING.load(Ordering::SeqCst) {
        return;
    }
    A1111_WAS_RUNNING.store(false, Ordering::SeqCst);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    {
        let mut last = LAST_A1111_RELAUNCH_MS.lock().unwrap();
        if now_ms.saturating_sub(*last) < MIN_RESTART_GAP_MS {
            return;
        }
        *last = now_ms;
    }
    let manager = ctx.runtime_manager.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = manager.start_runtime(A1111_RUNTIME_ID).await {
            println!("[gpu-guard] A1111 relaunch failed: {error}");
        }
    });
}

fn manager_suppress_clear(ctx: &GpuGuardContext) {
    let manager = ctx.runtime_manager.clone();
    tauri::async_runtime::block_on(async move {
        manager.suppress_runtime(A1111_RUNTIME_ID, false).await;
    });
}

/// RAII guard for one (or one batch of) Hunyuan generation(s).
pub struct GenerationGuard {
    _private: (),
}

impl GenerationGuard {
    pub fn acquire() -> Self {
        let previous = ACTIVE_GENERATIONS.fetch_add(1, Ordering::SeqCst);
        if previous == 0
            && let Some(ctx) = CTX.get()
        {
            A1111_WAS_RUNNING.store(a1111_listening(), Ordering::SeqCst);
            park_a1111(ctx);
        }
        Self { _private: () }
    }
}

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        let previous = ACTIVE_GENERATIONS.fetch_sub(1, Ordering::SeqCst);
        if previous == 1
            && let Some(ctx) = CTX.get()
        {
            relaunch_a1111_if_needed(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_nests_and_releases() {
        // Without an installed context the guard must still be safe.
        let outer = GenerationGuard::acquire();
        assert_eq!(ACTIVE_GENERATIONS.load(Ordering::SeqCst), 1);
        let inner = GenerationGuard::acquire();
        assert_eq!(ACTIVE_GENERATIONS.load(Ordering::SeqCst), 2);
        drop(inner);
        assert_eq!(ACTIVE_GENERATIONS.load(Ordering::SeqCst), 1);
        drop(outer);
        assert_eq!(ACTIVE_GENERATIONS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a1111_port_probe_handles_closed_port() {
        // Must not panic whether or not something listens on 7860.
        let _ = a1111_listening();
    }
}

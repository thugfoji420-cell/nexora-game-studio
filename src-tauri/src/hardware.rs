use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const HARDWARE_SCHEMA_VERSION: u32 = 1;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_OUTPUT: usize = 256 * 1024;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuSnapshot {
    pub name: String,
    pub vendor: String,
    pub vram_total_mib: Option<u64>,
    pub vram_used_mib: Option<u64>,
    pub vram_free_mib: Option<u64>,
    pub driver: Option<String>,
    pub source: String,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareSource {
    pub name: String,
    pub confidence: Confidence,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareSnapshot {
    pub schema_version: u32,
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_logical_count: usize,
    pub cpu_physical_count: Option<u32>,
    pub ram_total_mib: Option<u64>,
    pub gpus: Vec<GpuSnapshot>,
    pub active_project_disk_free_mib: Option<u64>,
    pub captured_at: DateTime<Utc>,
    pub sources: Vec<HardwareSource>,
    pub confidence: Confidence,
}

impl HardwareSnapshot {
    pub fn unknown() -> Self {
        Self {
            schema_version: HARDWARE_SCHEMA_VERSION,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_model: "unknown".to_string(),
            cpu_logical_count: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            cpu_physical_count: None,
            ram_total_mib: None,
            gpus: Vec::new(),
            active_project_disk_free_mib: None,
            captured_at: Utc::now(),
            sources: vec![HardwareSource {
                name: "rust_std".to_string(),
                confidence: Confidence::Medium,
            }],
            confidence: Confidence::Unknown,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CimSnapshot {
    os_caption: Option<String>,
    cpu_model: Option<String>,
    logical_count: Option<u64>,
    physical_count: Option<u64>,
    ram_bytes: Option<u64>,
    disk_free_bytes: Option<u64>,
    #[serde(default)]
    gpus: Vec<CimGpu>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CimGpu {
    name: Option<String>,
    adapter_ram: Option<u64>,
    driver_version: Option<String>,
    video_processor: Option<String>,
}

const WINDOWS_PROBE_SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
$projectPath=$args[0]
$cpu=@(Get-CimInstance Win32_Processor)
$cs=Get-CimInstance Win32_ComputerSystem
$os=Get-CimInstance Win32_OperatingSystem
$gpu=@(Get-CimInstance Win32_VideoController | ForEach-Object { [pscustomobject]@{ Name=$_.Name; AdapterRam=$_.AdapterRAM; DriverVersion=$_.DriverVersion; VideoProcessor=$_.VideoProcessor } })
$diskFree=$null
if ($projectPath) { $root=[System.IO.Path]::GetPathRoot($projectPath); if ($root) { $escaped=$root.Replace("'","''"); $disk=Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($escaped.TrimEnd('\'))'"; if ($disk) { $diskFree=[uint64]$disk.FreeSpace } } }
[pscustomobject]@{ OsCaption=$os.Caption; CpuModel=($cpu | Select-Object -First 1).Name; LogicalCount=[uint64](($cpu | Measure-Object NumberOfLogicalProcessors -Sum).Sum); PhysicalCount=[uint64](($cpu | Measure-Object NumberOfCores -Sum).Sum); RamBytes=[uint64]$cs.TotalPhysicalMemory; DiskFreeBytes=$diskFree; Gpus=$gpu } | ConvertTo-Json -Depth 4 -Compress
"#;

pub fn detect(active_project: Option<&Path>) -> HardwareSnapshot {
    let mut snapshot = HardwareSnapshot::unknown();
    if cfg!(windows) {
        let project = active_project
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let args = [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            WINDOWS_PROBE_SCRIPT,
            &project,
        ];
        if let Ok(output) = run_bounded("powershell.exe", &args, PROBE_TIMEOUT, MAX_PROBE_OUTPUT)
            && output.status_success
            && let Ok(cim) = parse_cim(&output.stdout)
        {
            apply_cim(&mut snapshot, cim);
        }

        let nvidia_args = [
            "--query-gpu=name,driver_version,memory.total,memory.used,memory.free",
            "--format=csv,noheader,nounits",
        ];
        if let Ok(output) = run_bounded(
            "nvidia-smi.exe",
            &nvidia_args,
            PROBE_TIMEOUT,
            MAX_PROBE_OUTPUT,
        ) && output.status_success
            && let Ok(gpus) = parse_nvidia_csv(&output.stdout)
            && !gpus.is_empty()
        {
            snapshot.gpus.retain(|gpu| gpu.vendor != "nvidia");
            snapshot.gpus.extend(gpus);
            snapshot.sources.push(HardwareSource {
                name: "nvidia_smi".to_string(),
                confidence: Confidence::High,
            });
        }
    }
    snapshot.captured_at = Utc::now();
    snapshot.confidence = aggregate_confidence(&snapshot);
    snapshot
}

fn parse_cim(bytes: &[u8]) -> Result<CimSnapshot, String> {
    let text = decode_command_output(bytes);
    serde_json::from_str(text.trim()).map_err(|error| format!("invalid CIM JSON: {error}"))
}

fn apply_cim(snapshot: &mut HardwareSnapshot, cim: CimSnapshot) {
    if let Some(os) = cim.os_caption.filter(|value| !value.trim().is_empty()) {
        snapshot.os = os.trim().to_string();
    }
    if let Some(cpu) = cim.cpu_model.filter(|value| !value.trim().is_empty()) {
        snapshot.cpu_model = cpu.trim().to_string();
    }
    if let Some(logical) = cim
        .logical_count
        .and_then(|value| usize::try_from(value).ok())
        && logical > 0
    {
        snapshot.cpu_logical_count = logical;
    }
    snapshot.cpu_physical_count = cim
        .physical_count
        .filter(|value| *value > 0)
        .and_then(|value| u32::try_from(value).ok());
    snapshot.ram_total_mib = cim
        .ram_bytes
        .filter(|value| *value > 0)
        .map(|value| value / MIB);
    snapshot.active_project_disk_free_mib = cim
        .disk_free_bytes
        .filter(|value| *value > 0)
        .map(|value| value / MIB);
    snapshot.gpus = cim
        .gpus
        .into_iter()
        .filter_map(|gpu| {
            let name = gpu.name?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(GpuSnapshot {
                vendor: infer_vendor(&format!(
                    "{} {}",
                    name,
                    gpu.video_processor.unwrap_or_default()
                )),
                name,
                vram_total_mib: gpu
                    .adapter_ram
                    .filter(|value| *value > 0)
                    .map(|value| value / MIB),
                vram_used_mib: None,
                vram_free_mib: None,
                driver: gpu.driver_version.filter(|value| !value.trim().is_empty()),
                source: "windows_cim".to_string(),
                confidence: Confidence::Low,
            })
        })
        .collect();
    snapshot.sources.push(HardwareSource {
        name: "windows_cim".to_string(),
        confidence: Confidence::Medium,
    });
}

fn infer_vendor(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if lower.contains("nvidia") {
        "nvidia"
    } else if lower.contains("amd") || lower.contains("radeon") || lower.contains("advanced micro")
    {
        "amd"
    } else if lower.contains("intel") {
        "intel"
    } else {
        "unknown"
    }
    .to_string()
}

fn parse_nvidia_csv(bytes: &[u8]) -> Result<Vec<GpuSnapshot>, String> {
    let text = decode_command_output(bytes);
    let mut result = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields =
            parse_csv_line(line).map_err(|error| format!("NVIDIA row {}: {error}", index + 1))?;
        if fields.len() != 5 {
            return Err(format!(
                "NVIDIA row {} has {} fields",
                index + 1,
                fields.len()
            ));
        }
        let parse_mib = |value: &str| {
            value
                .trim()
                .parse::<u64>()
                .map_err(|_| format!("invalid memory value {value:?}"))
        };
        let total = parse_mib(&fields[2])?;
        let used = parse_mib(&fields[3])?;
        let free = parse_mib(&fields[4])?;
        if used.saturating_add(free) > total.saturating_add(64) {
            return Err("inconsistent NVIDIA memory values".to_string());
        }
        result.push(GpuSnapshot {
            name: fields[0].trim().to_string(),
            vendor: "nvidia".to_string(),
            vram_total_mib: Some(total),
            vram_used_mib: Some(used),
            vram_free_mib: Some(free),
            driver: Some(fields[1].trim().to_string()),
            source: "nvidia_smi".to_string(),
            confidence: Confidence::High,
        });
    }
    Ok(result)
}

fn parse_csv_line(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(character),
        }
    }
    if quoted {
        return Err("unterminated quoted field".to_string());
    }
    fields.push(field);
    Ok(fields)
}

fn decode_command_output(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes)
            .trim_start_matches('\u{feff}')
            .to_string()
    }
}

fn aggregate_confidence(snapshot: &HardwareSnapshot) -> Confidence {
    if snapshot.cpu_model == "unknown" && snapshot.ram_total_mib.is_none() {
        Confidence::Unknown
    } else if snapshot.ram_total_mib.is_some() && snapshot.cpu_model != "unknown" {
        Confidence::High
    } else {
        Confidence::Medium
    }
}

struct ProcessOutput {
    status_success: bool,
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
}

fn run_bounded(
    executable: &str,
    args: &[&str],
    timeout: Duration,
    max_output: usize,
) -> io::Result<ProcessOutput> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = thread::spawn(move || read_bounded(stdout, max_output));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, max_output));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "hardware probe timed out",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("hardware probe stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("hardware probe stderr reader panicked"))??;
    Ok(ProcessOutput {
        status_success: status.success(),
        stdout,
        stderr,
    })
}

fn read_bounded(reader: impl Read, max_output: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take((max_output + 1) as u64)
        .read_to_end(&mut output)?;
    if output.len() > max_output {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hardware probe output exceeded limit",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIM: &str = r#"{"OsCaption":"Microsoft Windows 11 Pro","CpuModel":"Fixture CPU","LogicalCount":16,"PhysicalCount":8,"RamBytes":17179869184,"DiskFreeBytes":107374182400,"Gpus":[{"Name":"Intel UHD","AdapterRam":1073741824,"DriverVersion":"1.2","VideoProcessor":"Intel"}]}"#;

    #[test]
    fn cim_fixture_parses_cpu_ram_gpu_and_disk() {
        let mut snapshot = HardwareSnapshot::unknown();
        apply_cim(&mut snapshot, parse_cim(CIM.as_bytes()).unwrap());
        assert_eq!(snapshot.cpu_model, "Fixture CPU");
        assert_eq!(snapshot.cpu_logical_count, 16);
        assert_eq!(snapshot.cpu_physical_count, Some(8));
        assert_eq!(snapshot.ram_total_mib, Some(16_384));
        assert_eq!(snapshot.active_project_disk_free_mib, Some(102_400));
        assert_eq!(snapshot.gpus[0].vendor, "intel");
    }

    #[test]
    fn cim_unknowns_are_preserved() {
        let mut snapshot = HardwareSnapshot::unknown();
        apply_cim(&mut snapshot, parse_cim(br#"{"Gpus":[]}"#).unwrap());
        assert_eq!(snapshot.cpu_model, "unknown");
        assert_eq!(snapshot.ram_total_mib, None);
        assert_eq!(snapshot.cpu_physical_count, None);
    }

    #[test]
    fn nvidia_fixture_parses_quoted_name() {
        let gpus = parse_nvidia_csv(b"\"NVIDIA RTX, Pro\", 555.1, 24576, 1024, 23552\n").unwrap();
        assert_eq!(gpus[0].name, "NVIDIA RTX, Pro");
        assert_eq!(gpus[0].vram_free_mib, Some(23_552));
        assert_eq!(gpus[0].confidence, Confidence::High);
    }

    #[test]
    fn nvidia_fixture_rejects_bad_rows() {
        assert!(parse_nvidia_csv(b"GPU, driver, nope, 1, 2").is_err());
        assert!(parse_nvidia_csv(b"GPU, driver, 10, 100, 100").is_err());
        assert!(parse_nvidia_csv(b"\"GPU, driver, 10, 1, 9").is_err());
    }

    #[test]
    fn snapshot_schema_and_unknown_serialization() {
        let value = serde_json::to_value(HardwareSnapshot::unknown()).unwrap();
        assert_eq!(value["schemaVersion"], HARDWARE_SCHEMA_VERSION);
        assert!(value["ramTotalMib"].is_null());
        assert!(value["gpus"].as_array().unwrap().is_empty());
        assert!(value["capturedAt"].is_string());
    }

    #[test]
    fn utf16_powershell_output_decodes() {
        let mut bytes = vec![0xff, 0xfe];
        for word in "{\"Gpus\":[]}".encode_utf16() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        assert!(parse_cim(&bytes).is_ok());
    }

    #[test]
    fn vendor_inference_is_defensive() {
        assert_eq!(infer_vendor("AMD Radeon"), "amd");
        assert_eq!(infer_vendor("Mystery GPU"), "unknown");
    }
}

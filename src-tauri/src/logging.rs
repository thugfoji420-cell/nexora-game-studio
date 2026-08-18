use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct JsonLogger {
    path: PathBuf,
    file: Mutex<File>,
}

#[derive(Serialize)]
struct LogEntry<'a> {
    timestamp_ms: u128,
    level: &'a str,
    event: &'a str,
    message: &'a str,
}

impl JsonLogger {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, event: &str, message: &str) {
        self.write("INFO", event, message);
    }

    pub fn error(&self, event: &str, message: &str) {
        self.write("ERROR", event, message);
    }

    fn write(&self, level: &str, event: &str, message: &str) {
        let entry = LogEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            level,
            event,
            message,
        };
        if let (Ok(line), Ok(mut file)) = (serde_json::to_string(&entry), self.file.lock()) {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

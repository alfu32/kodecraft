use crate::jobs::Clock;
use kode_core::{DomainError, Result};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Bounded JSON-lines logging; never writes to stdout or the terminal canvas.
pub struct FileLog {
    path: PathBuf,
    file: Mutex<File>,
    max_bytes: u64,
}
impl FileLog {
    pub fn open(path: &Path, max_bytes: u64) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| DomainError::Io {
                operation: "create log directory",
                path: parent.into(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| DomainError::Io {
                operation: "open log",
                path: path.into(),
                source,
            })?;
        Ok(Self {
            path: path.into(),
            file: Mutex::new(file),
            max_bytes: max_bytes.max(4096),
        })
    }
    pub fn record(
        &self,
        clock: &dyn Clock,
        level: &str,
        component: &str,
        message: &str,
        fields: Value,
    ) -> Result<()> {
        let value = serde_json::json!({"time_ms": clock.now_millis(), "level": level, "component": component, "message": message.chars().take(4096).collect::<String>(), "fields": fields});
        let mut encoded = serde_json::to_vec(&value).map_err(|error| DomainError::Parse {
            format: "log record",
            message: error.to_string(),
        })?;
        if encoded.len() as u64 + 1 > self.max_bytes {
            return Err(DomainError::Unsupported(
                "log record exceeds log capacity".into(),
            ));
        }
        encoded.push(b'\n');
        let mut file = self.file.lock().expect("log mutex");
        let outcome = (|| -> std::io::Result<()> {
            if file.metadata()?.len() + encoded.len() as u64 > self.max_bytes {
                file.flush()?;
                // Copy then truncate works on Windows while the file is open.
                fs::copy(&self.path, self.path.with_extension("previous.jsonl"))?;
                file.set_len(0)?;
            }
            file.write_all(&encoded)?;
            file.flush()
        })();
        outcome.map_err(|source| DomainError::Io {
            operation: "write log",
            path: self.path.clone(),
            source,
        })
    }
}

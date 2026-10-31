//! A log of finished operations, newest first, optionally persisted as JSON
//! lines so the history survives restarts.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ops::OperationStatus;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRecord {
    pub id: Uuid,
    /// Operation type, e.g. `"copy"`.
    pub kind: String,
    pub summary: String,
    pub status: OperationStatus,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub files_done: u64,
    pub bytes_done: u64,
    pub skipped: u64,
    pub error: Option<String>,
}

pub struct ActivityLog {
    records: Mutex<VecDeque<ActivityRecord>>,
    capacity: usize,
    file: Option<PathBuf>,
}

impl ActivityLog {
    /// An in-memory log that keeps the newest `capacity` records.
    pub fn in_memory(capacity: usize) -> Self {
        Self {
            records: Mutex::new(VecDeque::new()),
            capacity: capacity.max(1),
            file: None,
        }
    }

    /// A log backed by a JSON-lines file. Existing records are loaded; lines
    /// that fail to parse are ignored.
    pub fn open(path: impl Into<PathBuf>, capacity: usize) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut records = VecDeque::new();
        if let Ok(f) = std::fs::File::open(&path) {
            for line in std::io::BufReader::new(f).lines() {
                if let Ok(r) = serde_json::from_str::<ActivityRecord>(&line?) {
                    records.push_front(r);
                }
            }
        }
        let capacity = capacity.max(1);
        records.truncate(capacity);
        let log = Self {
            records: Mutex::new(records),
            capacity,
            file: Some(path),
        };
        log.compact()?;
        Ok(log)
    }

    pub fn record(&self, r: ActivityRecord) {
        if let Some(path) = &self.file {
            // Logging must never fail an operation, so I/O errors are ignored.
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                if let Ok(line) = serde_json::to_string(&r) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        let mut recs = self.records.lock().unwrap();
        recs.push_front(r);
        recs.truncate(self.capacity);
    }

    /// Newest first.
    pub fn list(&self) -> Vec<ActivityRecord> {
        self.records.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear(&self) -> std::io::Result<()> {
        self.records.lock().unwrap().clear();
        self.compact()
    }

    /// Rewrite the backing file so it only holds the retained records.
    fn compact(&self) -> std::io::Result<()> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        let recs = self.records.lock().unwrap();
        let mut f = std::fs::File::create(path)?;
        for r in recs.iter().rev() {
            writeln!(
                f,
                "{}",
                serde_json::to_string(r).map_err(std::io::Error::other)?
            )?;
        }
        Ok(())
    }
}

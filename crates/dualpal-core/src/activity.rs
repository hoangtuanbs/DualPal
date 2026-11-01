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
    /// Ids of the file systems involved (missing in records from older versions).
    #[serde(default)]
    pub places: Vec<String>,
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

// ---- insights -------------------------------------------------------------------

/// Work done on one day (in the user's time zone).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayStat {
    /// Start of the day, in ms since the epoch.
    pub start_ms: i64,
    pub operations: u64,
    pub failed: u64,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupStat {
    /// Operation kind (`"copy"`) or file system id (`"local"`, `"conn:…"`).
    pub key: String,
    pub operations: u64,
    pub failed: u64,
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Insights {
    pub days: Vec<DayStat>,
    pub operations: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub files: u64,
    pub bytes: u64,
    /// Time spent running operations.
    pub busy_ms: i64,
    /// Average speed of copies, moves and syncs, in bytes per second.
    pub throughput: Option<f64>,
    pub by_kind: Vec<GroupStat>,
    pub by_place: Vec<GroupStat>,
    /// The biggest transfers, largest first.
    pub largest: Vec<ActivityRecord>,
    /// The latest failures, newest first.
    pub failures: Vec<ActivityRecord>,
}

const DAY_MS: i64 = 86_400_000;

/// Statistics over the last `days` days, counted in a time zone `utc_offset_minutes`
/// ahead of UTC.
pub fn insights(
    records: &[ActivityRecord],
    now_ms: i64,
    days: u32,
    utc_offset_minutes: i32,
) -> Insights {
    let days = days.clamp(1, 366) as i64;
    let offset = utc_offset_minutes as i64 * 60_000;
    let day_of = |ms: i64| (ms + offset).div_euclid(DAY_MS);
    let today = day_of(now_ms);
    let first = today - (days - 1);
    let mut out = Insights {
        days: (first..=today)
            .map(|d| DayStat {
                start_ms: d * DAY_MS - offset,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut kinds: std::collections::HashMap<String, GroupStat> = Default::default();
    let mut places: std::collections::HashMap<String, GroupStat> = Default::default();
    let (mut moved_bytes, mut moved_ms) = (0u64, 0i64);
    let mut in_window: Vec<&ActivityRecord> = Vec::new();
    for r in records {
        let d = day_of(r.finished_ms);
        if d < first || d > today {
            continue;
        }
        in_window.push(r);
        let failed = u64::from(r.status == OperationStatus::Failed);
        let day = &mut out.days[(d - first) as usize];
        day.operations += 1;
        day.failed += failed;
        day.files += r.files_done;
        day.bytes += r.bytes_done;
        out.operations += 1;
        out.failed += failed;
        out.completed += u64::from(r.status == OperationStatus::Completed);
        out.cancelled += u64::from(r.status == OperationStatus::Cancelled);
        out.files += r.files_done;
        out.bytes += r.bytes_done;
        let ms = (r.finished_ms - r.started_ms).max(0);
        out.busy_ms += ms;
        if matches!(r.kind.as_str(), "copy" | "move" | "sync") && r.bytes_done > 0 && ms > 0 {
            moved_bytes += r.bytes_done;
            moved_ms += ms;
        }
        for (map, key) in [
            (&mut kinds, vec![r.kind.clone()]),
            (&mut places, r.places.clone()),
        ] {
            for k in key {
                let g = map.entry(k.clone()).or_insert_with(|| GroupStat {
                    key: k,
                    ..Default::default()
                });
                g.operations += 1;
                g.failed += failed;
                g.files += r.files_done;
                g.bytes += r.bytes_done;
            }
        }
    }
    if moved_ms > 0 {
        out.throughput = Some(moved_bytes as f64 / (moved_ms as f64 / 1000.0));
    }
    let sorted = |m: std::collections::HashMap<String, GroupStat>| {
        let mut v: Vec<GroupStat> = m.into_values().collect();
        v.sort_by(|a, b| (b.bytes, b.operations, &a.key).cmp(&(a.bytes, a.operations, &b.key)));
        v
    };
    out.by_kind = sorted(kinds);
    out.by_place = sorted(places);
    let mut largest: Vec<&ActivityRecord> = in_window
        .iter()
        .copied()
        .filter(|r| r.bytes_done > 0)
        .collect();
    largest.sort_by_key(|r| std::cmp::Reverse(r.bytes_done));
    out.largest = largest.into_iter().take(5).cloned().collect();
    let mut failures: Vec<&ActivityRecord> = in_window
        .iter()
        .copied()
        .filter(|r| r.status == OperationStatus::Failed)
        .collect();
    failures.sort_by_key(|r| std::cmp::Reverse(r.finished_ms));
    out.failures = failures.into_iter().take(5).cloned().collect();
    out
}

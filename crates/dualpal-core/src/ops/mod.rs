//! Background file operations with live progress.
//!
//! [`OperationQueue::enqueue`] returns immediately with an id. The operation
//! runs on the Tokio runtime (at most `max_concurrent` at a time) and every
//! state change is broadcast as an [`OperationInfo`] to subscribers, which
//! is what drives the progress view in the UI. Finished operations are also
//! written to the [`ActivityLog`].

mod exec;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::activity::{ActivityLog, ActivityRecord};
use crate::vfs::{Location, Registry};

/// What to do when a destination file already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Stop the operation with an `alreadyExists` error.
    #[default]
    Fail,
    /// Leave the existing file alone and continue.
    Skip,
    /// Replace the existing file.
    Overwrite,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Operation {
    /// Copy files or folders into the `destination` folder.
    Copy {
        sources: Vec<Location>,
        destination: Location,
        #[serde(default)]
        conflict: ConflictPolicy,
    },
    /// Move files or folders into the `destination` folder.
    Move {
        sources: Vec<Location>,
        destination: Location,
        #[serde(default)]
        conflict: ConflictPolicy,
    },
    /// Move files or folders to the system Trash / Recycle Bin.
    Trash { targets: Vec<Location> },
    /// Delete files or folders permanently (recursively).
    Delete { targets: Vec<Location> },
    /// Create a ZIP archive at `destination` holding the given items.
    Compress {
        sources: Vec<Location>,
        destination: Location,
    },
    /// Change Unix permission bits of one item.
    SetMode { location: Location, mode: u32 },
    /// Create a new folder at `location`.
    CreateDir { location: Location },
    /// Rename an item in place.
    Rename {
        location: Location,
        new_name: String,
    },
}

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Operation::Copy { .. } => "copy",
            Operation::Move { .. } => "move",
            Operation::Trash { .. } => "trash",
            Operation::Delete { .. } => "delete",
            Operation::Compress { .. } => "compress",
            Operation::SetMode { .. } => "setMode",
            Operation::CreateDir { .. } => "createDir",
            Operation::Rename { .. } => "rename",
        }
    }

    /// One-line description for the progress view and activity log.
    pub fn summary(&self) -> String {
        fn items(n: usize) -> String {
            if n == 1 {
                "1 item".into()
            } else {
                format!("{n} items")
            }
        }
        match self {
            Operation::Copy {
                sources,
                destination,
                ..
            } => {
                format!("Copy {} to {}", items(sources.len()), destination.path)
            }
            Operation::Move {
                sources,
                destination,
                ..
            } => {
                format!("Move {} to {}", items(sources.len()), destination.path)
            }
            Operation::Trash { targets } => format!("Move {} to Trash", items(targets.len())),
            Operation::Delete { targets } => format!("Delete {}", items(targets.len())),
            Operation::Compress {
                sources,
                destination,
            } => {
                format!(
                    "Compress {} into {}",
                    items(sources.len()),
                    destination.path
                )
            }
            Operation::SetMode { location, mode } => {
                format!("Set permissions of {} to {mode:o}", location.path)
            }
            Operation::CreateDir { location } => format!("New folder {}", location.path),
            Operation::Rename { location, new_name } => {
                format!("Rename {} to {new_name}", location.path)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl OperationStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub files_total: u64,
    pub files_done: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    /// Items left untouched because of [`ConflictPolicy::Skip`] or because
    /// they cannot be handled (e.g. special files).
    pub skipped: u64,
    /// Path of the item currently being processed.
    pub current: Option<String>,
}

/// Snapshot of an operation, broadcast on every change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationInfo {
    pub id: Uuid,
    pub kind: String,
    pub summary: String,
    pub status: OperationStatus,
    pub progress: Progress,
    pub error: Option<crate::error::ErrorInfo>,
    pub queued_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
}

struct Slot {
    info: OperationInfo,
    cancel: CancellationToken,
}

pub(crate) struct Inner {
    registry: Arc<Registry>,
    activity: Arc<ActivityLog>,
    slots: Mutex<HashMap<Uuid, Slot>>,
    events: broadcast::Sender<OperationInfo>,
    permits: Semaphore,
}

impl Inner {
    /// Apply `f` to the stored info and broadcast the result.
    fn update(&self, id: Uuid, f: impl FnOnce(&mut OperationInfo)) -> Option<OperationInfo> {
        let snapshot = {
            let mut slots = self.slots.lock().unwrap();
            let slot = slots.get_mut(&id)?;
            f(&mut slot.info);
            slot.info.clone()
        };
        let _ = self.events.send(snapshot.clone());
        Some(snapshot)
    }
}

#[derive(Clone)]
pub struct OperationQueue {
    inner: Arc<Inner>,
}

impl OperationQueue {
    pub fn new(registry: Arc<Registry>, activity: Arc<ActivityLog>, max_concurrent: usize) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                registry,
                activity,
                slots: Mutex::new(HashMap::new()),
                events,
                permits: Semaphore::new(max_concurrent.max(1)),
            }),
        }
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.inner.registry
    }

    pub fn activity(&self) -> &Arc<ActivityLog> {
        &self.inner.activity
    }

    /// Receive a snapshot every time any operation changes.
    pub fn subscribe(&self) -> broadcast::Receiver<OperationInfo> {
        self.inner.events.subscribe()
    }

    /// Queue an operation and start it as soon as a slot is free.
    /// Must be called from within a Tokio runtime.
    pub fn enqueue(&self, op: Operation) -> Uuid {
        let id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let info = OperationInfo {
            id,
            kind: op.kind().to_string(),
            summary: op.summary(),
            status: OperationStatus::Queued,
            progress: Progress::default(),
            error: None,
            queued_ms: crate::now_ms(),
            started_ms: None,
            finished_ms: None,
        };
        self.inner.slots.lock().unwrap().insert(
            id,
            Slot {
                info: info.clone(),
                cancel: cancel.clone(),
            },
        );
        let _ = self.inner.events.send(info);

        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = tokio::select! {
                p = inner.permits.acquire() => p.expect("semaphore never closed"),
                _ = cancel.cancelled() => {
                    finish(&inner, id, Err(crate::Error::Cancelled));
                    return;
                }
            };
            inner.update(id, |i| {
                i.status = OperationStatus::Running;
                i.started_ms = Some(crate::now_ms());
            });
            let result = exec::run(inner.clone(), id, cancel, op).await;
            finish(&inner, id, result);
        });
        id
    }

    /// Request cancellation. Returns false if the id is unknown or finished.
    pub fn cancel(&self, id: Uuid) -> bool {
        let slots = self.inner.slots.lock().unwrap();
        match slots.get(&id) {
            Some(s) if !s.info.status.is_finished() => {
                s.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    pub fn get(&self, id: Uuid) -> Option<OperationInfo> {
        self.inner
            .slots
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.info.clone())
    }

    /// All operations still in memory, oldest first.
    pub fn list(&self) -> Vec<OperationInfo> {
        let mut v: Vec<_> = self
            .inner
            .slots
            .lock()
            .unwrap()
            .values()
            .map(|s| s.info.clone())
            .collect();
        v.sort_by_key(|i| i.queued_ms);
        v
    }

    /// Forget finished operations (they remain in the activity log).
    pub fn clear_finished(&self) {
        self.inner
            .slots
            .lock()
            .unwrap()
            .retain(|_, s| !s.info.status.is_finished());
    }

    /// Wait until the operation has finished and return its final state.
    pub async fn wait(&self, id: Uuid) -> Option<OperationInfo> {
        let mut rx = self.subscribe();
        loop {
            let info = self.get(id)?;
            if info.status.is_finished() {
                return Some(info);
            }
            match rx.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return self.get(id),
            }
        }
    }
}

fn finish(inner: &Inner, id: Uuid, result: crate::Result<()>) {
    let Some(info) = inner.update(id, |i| {
        i.finished_ms = Some(crate::now_ms());
        i.progress.current = None;
        match result {
            Ok(()) => i.status = OperationStatus::Completed,
            Err(crate::Error::Cancelled) => i.status = OperationStatus::Cancelled,
            Err(e) => {
                i.status = OperationStatus::Failed;
                i.error = Some((&e).into());
            }
        }
    }) else {
        return;
    };
    inner.activity.record(ActivityRecord {
        id,
        kind: info.kind.clone(),
        summary: info.summary.clone(),
        status: info.status,
        started_ms: info.started_ms.unwrap_or(info.queued_ms),
        finished_ms: info.finished_ms.unwrap_or_default(),
        files_done: info.progress.files_done,
        bytes_done: info.progress.bytes_done,
        skipped: info.progress.skipped,
        error: info.error.as_ref().map(|e| e.message.clone()),
    });
}

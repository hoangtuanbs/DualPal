//! Background file operations with live progress.
//!
//! [`OperationQueue::enqueue`] returns immediately with an id. The operation
//! runs on the Tokio runtime (at most `max_concurrent` at a time) and every
//! state change is broadcast as an [`OperationInfo`] to subscribers, which
//! is what drives the progress view in the UI. Finished operations are also
//! written to the [`ActivityLog`].
//!
//! Operations can be paused and resumed one by one or all together, copies
//! can be limited in bandwidth, checked after writing and told not to stop
//! on errors (see [`TransferOptions`]). With a store (see
//! [`OperationQueue::with_store`]) unfinished copies and moves are kept
//! across restarts and come back paused.

mod exec;
mod limit;
mod store;

pub use limit::RateLimiter;
pub use store::SavedOperation;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, OwnedSemaphorePermit, Semaphore};
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
    /// Rename several items at once (Multi Rename). Names may be swapped.
    BatchRename {
        items: Vec<crate::rename::RenameItem>,
    },
    /// Carry out the steps of a folder sync between `left` and `right`.
    Sync {
        left: Location,
        right: Location,
        steps: Vec<crate::sync::SyncStep>,
    },
    /// Copy or move each item to its own target path (used to retry the
    /// items that failed in an earlier copy or move). All targets are on one
    /// file system; missing parent folders are created.
    CopyItems {
        items: Vec<ItemPair>,
        #[serde(default)]
        is_move: bool,
        #[serde(default)]
        conflict: ConflictPolicy,
    },
}

/// A source and the exact path it is copied to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPair {
    pub source: Location,
    pub target: Location,
}

/// Per-operation choices for copies and moves. The defaults keep the
/// queue's usual behaviour: stop on the first error, no check, the global
/// bandwidth limit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransferOptions {
    /// Don't stop on errors: skip items that fail, list them at the end.
    pub keep_going: bool,
    /// Check each copied file against its source: by checksum, or by size
    /// where reading back is expensive (see [`crate::vfs::FileSystem::checksum_reads`]).
    pub verify: bool,
    /// Bandwidth limit for this operation in bytes per second. Empty means
    /// the queue's global limit, 0 means none.
    pub limit: Option<u64>,
}

/// An item a copy or move could not handle, with the reason.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedItem {
    pub source: Location,
    pub target: Location,
    pub error: crate::error::ErrorInfo,
}

/// The failures as text, one `path: reason` per line, to copy or save.
pub fn failure_report(items: &[FailedItem]) -> String {
    let mut out = String::new();
    for f in items {
        out.push_str(&f.source.path);
        out.push_str(": ");
        out.push_str(&f.error.message);
        out.push('\n');
    }
    out
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
            Operation::BatchRename { .. } => "batchRename",
            Operation::Sync { .. } => "sync",
            Operation::CopyItems { is_move: true, .. } => "move",
            Operation::CopyItems { .. } => "copy",
        }
    }

    /// Copies and moves: the operations kept across restarts.
    pub fn is_transfer(&self) -> bool {
        matches!(
            self,
            Operation::Copy { .. } | Operation::Move { .. } | Operation::CopyItems { .. }
        )
    }

    fn conflict(&self) -> ConflictPolicy {
        match self {
            Operation::Copy { conflict, .. }
            | Operation::Move { conflict, .. }
            | Operation::CopyItems { conflict, .. } => *conflict,
            _ => ConflictPolicy::Fail,
        }
    }

    /// Ids of the file systems this operation touches, for activity insights.
    pub fn places(&self) -> Vec<String> {
        let mut v: Vec<String> = match self {
            Operation::Copy {
                sources,
                destination,
                ..
            }
            | Operation::Move {
                sources,
                destination,
                ..
            }
            | Operation::Compress {
                sources,
                destination,
            } => sources
                .iter()
                .chain(std::iter::once(destination))
                .map(|l| l.fs.clone())
                .collect(),
            Operation::Trash { targets } | Operation::Delete { targets } => {
                targets.iter().map(|l| l.fs.clone()).collect()
            }
            Operation::SetMode { location, .. }
            | Operation::CreateDir { location }
            | Operation::Rename { location, .. } => vec![location.fs.clone()],
            Operation::BatchRename { items } => {
                items.iter().map(|i| i.location.fs.clone()).collect()
            }
            Operation::Sync { left, right, .. } => vec![left.fs.clone(), right.fs.clone()],
            Operation::CopyItems { items, .. } => items
                .iter()
                .flat_map(|i| [i.source.fs.clone(), i.target.fs.clone()])
                .collect(),
        };
        v.sort();
        v.dedup();
        v
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
            Operation::BatchRename { items: list } => format!("Rename {}", items(list.len())),
            Operation::Sync { left, right, steps } => format!(
                "Sync {} and {} ({})",
                left.path,
                right.path,
                if steps.len() == 1 {
                    "1 change".to_string()
                } else {
                    format!("{} changes", steps.len())
                }
            ),
            Operation::CopyItems {
                items: list,
                is_move,
                ..
            } => format!(
                "{} {} again",
                if *is_move { "Move" } else { "Copy" },
                items(list.len())
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Queued,
    Running,
    /// Paused by the user; waits until resumed.
    Paused,
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
    /// Ids of the file systems involved.
    #[serde(default)]
    pub places: Vec<String>,
    #[serde(default)]
    pub options: TransferOptions,
    /// Items that failed and were skipped (with [`TransferOptions::keep_going`]);
    /// [`OperationQueue::failed_items`] lists them.
    #[serde(default)]
    pub failed: u64,
    /// Id of a file system that is not connected and that this operation
    /// waits for.
    #[serde(default)]
    pub waiting_for: Option<String>,
    /// Kept from an earlier run of the app: waits for Resume or Discard.
    #[serde(default)]
    pub restored: bool,
}

/// [`Control::limit`] value meaning "use the queue's global limit".
const LIMIT_GLOBAL: u64 = u64::MAX;

/// What the user can change on an operation while it runs.
pub(crate) struct Control {
    pause: watch::Sender<bool>,
    /// [`LIMIT_GLOBAL`], 0 for no limit, or bytes per second.
    limit: AtomicU64,
    /// This operation's own bucket, used when `limit` is its own.
    bucket: RateLimiter,
}

impl Control {
    fn new(paused: bool, limit: Option<u64>) -> Self {
        let limit = limit.unwrap_or(LIMIT_GLOBAL);
        Self {
            pause: watch::channel(paused).0,
            limit: AtomicU64::new(limit),
            bucket: RateLimiter::new(if limit == LIMIT_GLOBAL { 0 } else { limit }),
        }
    }

    fn paused(&self) -> bool {
        *self.pause.borrow()
    }

    fn set_limit(&self, limit: Option<u64>) {
        let limit = limit.unwrap_or(LIMIT_GLOBAL);
        self.limit.store(limit, Ordering::SeqCst);
        if limit != LIMIT_GLOBAL {
            self.bucket.set_rate(limit);
        }
    }
}

struct Slot {
    info: OperationInfo,
    cancel: CancellationToken,
    ctl: Arc<Control>,
    /// A restored operation that has not started yet: it starts on resume.
    pending: Option<Operation>,
    conflict: ConflictPolicy,
}

pub(crate) struct Inner {
    registry: Arc<Registry>,
    activity: Arc<ActivityLog>,
    slots: Mutex<HashMap<Uuid, Slot>>,
    events: broadcast::Sender<OperationInfo>,
    permits: Arc<Semaphore>,
    /// What each sync operation did, collected until the sync engine takes it.
    sync_results: Mutex<HashMap<Uuid, Vec<crate::sync::SyncDone>>>,
    /// Pauses every operation at once.
    queue_pause: watch::Sender<bool>,
    /// The global bandwidth limit shared by all running transfers.
    limiter: RateLimiter,
    failures: Mutex<HashMap<Uuid, Vec<FailedItem>>>,
    store: store::QueueStore,
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

    /// Like [`Inner::update`], but only broadcasts when something changed.
    fn update_if(&self, id: Uuid, f: impl FnOnce(&mut OperationInfo)) {
        let snapshot = {
            let mut slots = self.slots.lock().unwrap();
            let Some(slot) = slots.get_mut(&id) else {
                return;
            };
            let before = (slot.info.status, slot.info.waiting_for.clone());
            f(&mut slot.info);
            if before == (slot.info.status, slot.info.waiting_for.clone()) {
                return;
            }
            slot.info.clone()
        };
        let _ = self.events.send(snapshot);
    }

    fn queue_paused(&self) -> bool {
        *self.queue_pause.borrow()
    }

    fn missing_place(&self, places: &[String]) -> Option<String> {
        places.iter().find(|p| !self.registry.contains(p)).cloned()
    }

    pub(crate) fn add_failure(&self, id: Uuid, item: FailedItem) {
        let n = {
            let mut f = self.failures.lock().unwrap();
            let list = f.entry(id).or_default();
            list.push(item);
            list.len() as u64
        };
        self.update(id, |i| i.failed = n);
    }
}

#[derive(Clone)]
pub struct OperationQueue {
    inner: Arc<Inner>,
}

impl OperationQueue {
    pub fn new(registry: Arc<Registry>, activity: Arc<ActivityLog>, max_concurrent: usize) -> Self {
        Self::build(
            registry,
            activity,
            max_concurrent,
            store::QueueStore::in_memory(),
        )
    }

    /// A queue that keeps unfinished copies and moves in `path` (a JSON
    /// file), so that [`OperationQueue::restore`] can bring them back after
    /// a restart.
    pub fn with_store(
        registry: Arc<Registry>,
        activity: Arc<ActivityLog>,
        max_concurrent: usize,
        path: impl Into<PathBuf>,
    ) -> Self {
        let store = store::QueueStore::open(path.into());
        Self::build(registry, activity, max_concurrent, store)
    }

    fn build(
        registry: Arc<Registry>,
        activity: Arc<ActivityLog>,
        max_concurrent: usize,
        store: store::QueueStore,
    ) -> Self {
        let (events, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                registry,
                activity,
                slots: Mutex::new(HashMap::new()),
                events,
                permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
                sync_results: Mutex::new(HashMap::new()),
                queue_pause: watch::channel(false).0,
                limiter: RateLimiter::new(0),
                failures: Mutex::new(HashMap::new()),
                store,
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
        self.enqueue_with(op, TransferOptions::default())
    }

    /// [`OperationQueue::enqueue`] with options for a copy or move (other
    /// operations ignore them).
    pub fn enqueue_with(&self, op: Operation, options: TransferOptions) -> Uuid {
        let id = Uuid::new_v4();
        let info = new_info(id, &op, options.clone(), crate::now_ms());
        if op.is_transfer() {
            self.inner.store.insert(SavedOperation {
                id,
                operation: op.clone(),
                options: options.clone(),
                queued_ms: info.queued_ms,
                parts: Default::default(),
            });
        }
        let cancel = CancellationToken::new();
        let ctl = Arc::new(Control::new(false, options.limit));
        self.inner.slots.lock().unwrap().insert(
            id,
            Slot {
                info: info.clone(),
                cancel: cancel.clone(),
                ctl: ctl.clone(),
                pending: None,
                conflict: op.conflict(),
            },
        );
        let _ = self.inner.events.send(info);
        spawn(self.inner.clone(), id, cancel, ctl, op, false);
        id
    }

    /// Bring back the operations saved by an earlier run, paused and marked
    /// as restored. Nothing runs until [`OperationQueue::resume`]. Returns
    /// how many were restored. Needs no runtime.
    pub fn restore(&self) -> usize {
        let mut n = 0;
        for saved in self.inner.store.all() {
            if self.inner.slots.lock().unwrap().contains_key(&saved.id) {
                continue;
            }
            let mut info = new_info(
                saved.id,
                &saved.operation,
                saved.options.clone(),
                saved.queued_ms,
            );
            info.status = OperationStatus::Paused;
            info.restored = true;
            info.waiting_for = self.inner.missing_place(&info.places);
            self.inner.slots.lock().unwrap().insert(
                saved.id,
                Slot {
                    info: info.clone(),
                    cancel: CancellationToken::new(),
                    ctl: Arc::new(Control::new(true, saved.options.limit)),
                    conflict: saved.operation.conflict(),
                    pending: Some(saved.operation),
                },
            );
            let _ = self.inner.events.send(info);
            n += 1;
        }
        n
    }

    /// Recompute which restored operations wait for a server, after one
    /// connected or disconnected. Running operations notice by themselves.
    pub fn refresh_waiting(&self) {
        let ids: Vec<(Uuid, Vec<String>)> = self
            .inner
            .slots
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.pending.is_some())
            .map(|(id, s)| (*id, s.info.places.clone()))
            .collect();
        for (id, places) in ids {
            let missing = self.inner.missing_place(&places);
            self.inner.update_if(id, |i| i.waiting_for = missing);
        }
    }

    /// Request cancellation. Returns false if the id is unknown or finished.
    pub fn cancel(&self, id: Uuid) -> bool {
        let pending = {
            let mut slots = self.inner.slots.lock().unwrap();
            match slots.get_mut(&id) {
                Some(s) if !s.info.status.is_finished() => {
                    s.cancel.cancel();
                    s.pending
                        .take()
                        .map(|op| (op, s.cancel.clone(), s.ctl.clone()))
                }
                _ => return false,
            }
        };
        // A restored operation that never started: run its task so that it
        // removes the partial files it left, then finishes as cancelled.
        if let Some((op, cancel, ctl)) = pending {
            if tokio::runtime::Handle::try_current().is_ok() {
                spawn(self.inner.clone(), id, cancel, ctl, op, true);
            } else {
                finish(&self.inner, id, Err(crate::Error::Cancelled));
            }
        }
        true
    }

    /// Pause an operation: it finishes the chunk it is writing, then waits
    /// (and gives its slot to the next operation). Returns false if the id
    /// is unknown or finished.
    pub fn pause(&self, id: Uuid) -> bool {
        let ctl = {
            let slots = self.inner.slots.lock().unwrap();
            match slots.get(&id) {
                Some(s) if !s.info.status.is_finished() => s.ctl.clone(),
                _ => return false,
            }
        };
        ctl.pause.send_replace(true);
        true
    }

    /// Resume a paused operation; a restored one starts (carrying on with
    /// partly copied files where the target allows). Must be called from
    /// within a Tokio runtime.
    pub fn resume(&self, id: Uuid) -> bool {
        let (ctl, pending, cancel) = {
            let mut slots = self.inner.slots.lock().unwrap();
            match slots.get_mut(&id) {
                Some(s) if !s.info.status.is_finished() => {
                    (s.ctl.clone(), s.pending.take(), s.cancel.clone())
                }
                _ => return false,
            }
        };
        ctl.pause.send_replace(false);
        if let Some(op) = pending {
            self.inner
                .update(id, |i| i.status = OperationStatus::Queued);
            spawn(self.inner.clone(), id, cancel, ctl, op, true);
        }
        true
    }

    /// Pause or resume every operation at once. Restored operations stay
    /// paused until resumed one by one.
    pub fn set_queue_paused(&self, paused: bool) {
        self.inner.queue_pause.send_replace(paused);
    }

    pub fn queue_paused(&self) -> bool {
        self.inner.queue_paused()
    }

    /// The global bandwidth limit in bytes per second, shared by all running
    /// transfers without their own limit. 0 turns it off.
    pub fn set_bandwidth_limit(&self, bytes_per_sec: u64) {
        self.inner.limiter.set_rate(bytes_per_sec);
    }

    pub fn bandwidth_limit(&self) -> u64 {
        self.inner.limiter.rate()
    }

    /// Give one operation its own limit (bytes per second, 0 for none), or
    /// put it back on the global one (`None`).
    pub fn set_operation_limit(&self, id: Uuid, limit: Option<u64>) -> bool {
        let ctl = {
            let mut slots = self.inner.slots.lock().unwrap();
            match slots.get_mut(&id) {
                Some(s) if !s.info.status.is_finished() => s.ctl.clone(),
                _ => return false,
            }
        };
        ctl.set_limit(limit);
        self.inner.update(id, |i| i.options.limit = limit);
        true
    }

    /// The items an operation skipped because they failed.
    pub fn failed_items(&self, id: Uuid) -> Vec<FailedItem> {
        self.inner
            .failures
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    /// Start a new operation that copies (or moves) again the items that
    /// failed in `id`, with the same options. `conflict` overrides the
    /// original conflict policy. None if nothing failed.
    pub fn retry_failed(&self, id: Uuid, conflict: Option<ConflictPolicy>) -> Option<Uuid> {
        let items = self.failed_items(id);
        if items.is_empty() {
            return None;
        }
        let (kind, options, original) = {
            let slots = self.inner.slots.lock().unwrap();
            let s = slots.get(&id)?;
            (s.info.kind.clone(), s.info.options.clone(), s.conflict)
        };
        let op = Operation::CopyItems {
            items: items
                .into_iter()
                .map(|f| ItemPair {
                    source: f.source,
                    target: f.target,
                })
                .collect(),
            is_move: kind == "move",
            conflict: conflict.unwrap_or(original),
        };
        Some(self.enqueue_with(op, options))
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
        let mut gone = Vec::new();
        self.inner.slots.lock().unwrap().retain(|id, s| {
            let keep = !s.info.status.is_finished();
            if !keep {
                gone.push(*id);
            }
            keep
        });
        let mut f = self.inner.failures.lock().unwrap();
        for id in gone {
            f.remove(&id);
        }
    }

    /// What a finished sync operation did (empty for other operations).
    pub fn take_sync_results(&self, id: Uuid) -> Vec<crate::sync::SyncDone> {
        self.inner
            .sync_results
            .lock()
            .unwrap()
            .remove(&id)
            .unwrap_or_default()
    }

    /// Wait until the operation has finished and return its final state.
    pub async fn wait(&self, id: Uuid) -> Option<OperationInfo> {
        self.wait_for(id, |i| i.status.is_finished()).await
    }

    /// Wait until the operation's state satisfies `done` and return it.
    /// None if the id is unknown.
    pub async fn wait_for(
        &self,
        id: Uuid,
        done: impl Fn(&OperationInfo) -> bool,
    ) -> Option<OperationInfo> {
        let mut rx = self.subscribe();
        loop {
            let info = self.get(id)?;
            if done(&info) {
                return Some(info);
            }
            match rx.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return self.get(id),
            }
        }
    }
}

fn new_info(id: Uuid, op: &Operation, options: TransferOptions, queued_ms: i64) -> OperationInfo {
    OperationInfo {
        id,
        kind: op.kind().to_string(),
        summary: op.summary(),
        status: OperationStatus::Queued,
        progress: Progress::default(),
        error: None,
        queued_ms,
        started_ms: None,
        finished_ms: None,
        places: op.places(),
        options,
        failed: 0,
        waiting_for: None,
        restored: false,
    }
}

/// Run `op` on the runtime once it may start. `resumed` marks a restored
/// operation: it waits for its servers to connect and carries on with the
/// files it had copied before.
fn spawn(
    inner: Arc<Inner>,
    id: Uuid,
    cancel: CancellationToken,
    ctl: Arc<Control>,
    op: Operation,
    resumed: bool,
) {
    tokio::spawn(async move {
        let result = match wait_to_start(&inner, id, &cancel, &ctl, &op, resumed).await {
            None => Err(crate::Error::Cancelled),
            Some(permit) => {
                inner.update(id, |i| {
                    i.status = OperationStatus::Running;
                    i.waiting_for = None;
                    i.started_ms.get_or_insert_with(crate::now_ms);
                });
                let job = exec::JobParts {
                    ctl,
                    resumed,
                    permit,
                };
                exec::run(inner.clone(), id, cancel, op, job).await
            }
        };
        remove_parts(&inner, id).await;
        finish(&inner, id, result);
    });
}

/// Wait while the operation (or the queue) is paused, its servers are not
/// connected (restored operations only), or no slot is free. None when it
/// was cancelled meanwhile.
async fn wait_to_start(
    inner: &Arc<Inner>,
    id: Uuid,
    cancel: &CancellationToken,
    ctl: &Control,
    op: &Operation,
    resumed: bool,
) -> Option<OwnedSemaphorePermit> {
    let mut op_rx = ctl.pause.subscribe();
    let mut q_rx = inner.queue_pause.subscribe();
    let mut reg_rx = inner.registry.subscribe();
    let places = op.places();
    loop {
        if cancel.is_cancelled() {
            return None;
        }
        let paused = *op_rx.borrow_and_update();
        let queue_paused = *q_rx.borrow_and_update();
        reg_rx.borrow_and_update();
        let missing = if resumed {
            inner.missing_place(&places)
        } else {
            None
        };
        inner.update_if(id, |i| {
            i.status = if paused {
                OperationStatus::Paused
            } else {
                OperationStatus::Queued
            };
            i.waiting_for = missing.clone();
        });
        if !paused && !queue_paused && missing.is_none() {
            tokio::select! {
                p = inner.permits.clone().acquire_owned() => {
                    let p = p.expect("semaphore never closed");
                    if ctl.paused() || inner.queue_paused() {
                        continue;
                    }
                    if resumed && inner.missing_place(&places).is_some() {
                        continue;
                    }
                    return Some(p);
                }
                _ = op_rx.changed() => {}
                _ = q_rx.changed() => {}
                _ = cancel.cancelled() => return None,
            }
        } else {
            tokio::select! {
                _ = op_rx.changed() => {}
                _ = q_rx.changed() => {}
                _ = reg_rx.changed() => {}
                _ = cancel.cancelled() => return None,
            }
        }
    }
}

/// Remove the temporary files an unfinished operation left (a cancelled or
/// discarded restored copy). Files on servers that are not connected stay.
async fn remove_parts(inner: &Inner, id: Uuid) {
    let Some(saved) = inner.store.get(id) else {
        return;
    };
    let Some(fs) = saved
        .destination_fs()
        .and_then(|f| inner.registry.get(f).ok())
    else {
        return;
    };
    for part in saved.parts.values() {
        let _ = fs.remove_file(part).await;
    }
}

fn finish(inner: &Inner, id: Uuid, result: crate::Result<()>) {
    inner.store.remove(id);
    let Some(info) = inner.update(id, |i| {
        i.finished_ms = Some(crate::now_ms());
        i.progress.current = None;
        i.waiting_for = None;
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
    let error = info.error.as_ref().map(|e| e.message.clone()).or_else(|| {
        (info.failed > 0).then(|| match info.failed {
            1 => "1 item failed".to_string(),
            n => format!("{n} items failed"),
        })
    });
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
        error,
        places: info.places.clone(),
    });
}

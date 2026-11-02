//! Auto-sync: keep two folders in sync while the user works, one way or
//! both ways, syncing again whenever something changes.
//!
//! Folders on this computer are watched for changes by the operating
//! system. Servers, buckets and Windows network or WSL folders send no
//! change notifications, so they are checked every few seconds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

use crate::compare::CompareOptions;
use crate::error::{Error, Result};
use crate::ops::{OperationQueue, OperationStatus};
use crate::sync::{SyncAction, SyncEngine, SyncMode};
use crate::vfs::{LocalFs, Location};

/// How often a side without change notifications is checked.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How often watched folders are checked anyway, in case a change
/// notification was missed.
const SAFETY_INTERVAL: Duration = Duration::from_secs(120);
/// Changes are collected this long before syncing, so that a burst of them
/// (a file being saved, a folder being unpacked) becomes one sync.
const SETTLE: Duration = Duration::from_millis(1500);
/// Wait before trying again after a sync fails.
const RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveDirection {
    /// Copy new and changed items from `from` to `to`; never deletes.
    OneWay,
    /// Copy changes both ways, and delete what was deleted on the other
    /// side since the last sync.
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveState {
    /// Comparing the folders and copying what changed.
    Syncing,
    /// In sync; waiting for the next change.
    Watching,
    /// The last sync failed; it is tried again shortly.
    Failed,
}

/// What auto-sync is doing, sent on every change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub from: Location,
    pub to: Location,
    pub direction: LiveDirection,
    pub state: LiveState,
    /// True for each side the system reports changes for; false for sides
    /// that are checked every [`POLL_INTERVAL`].
    pub from_watched: bool,
    pub to_watched: bool,
    /// When the folders were last found in sync, in ms since the epoch.
    pub last_sync_ms: Option<i64>,
    /// Items copied or deleted since auto-sync started.
    pub items_synced: u64,
    pub syncs: u64,
    pub error: Option<String>,
    /// Items changed on both sides, which two-way sync leaves alone.
    pub conflicts: u64,
}

struct Running {
    cancel: CancellationToken,
    status: Arc<Mutex<LiveStatus>>,
}

/// Runs at most one auto-sync at a time.
pub struct LiveSync {
    engine: Arc<SyncEngine>,
    queue: OperationQueue,
    tx: broadcast::Sender<Option<LiveStatus>>,
    running: Mutex<Option<Running>>,
}

impl LiveSync {
    pub fn new(engine: Arc<SyncEngine>, queue: OperationQueue) -> Arc<Self> {
        let (tx, _) = broadcast::channel(64);
        Arc::new(Self {
            engine,
            queue,
            tx,
            running: Mutex::new(None),
        })
    }

    /// Status updates; `None` once auto-sync stops.
    pub fn subscribe(&self) -> broadcast::Receiver<Option<LiveStatus>> {
        self.tx.subscribe()
    }

    pub fn status(&self) -> Option<LiveStatus> {
        let running = self.running.lock().unwrap();
        running.as_ref().map(|r| r.status.lock().unwrap().clone())
    }

    /// Start keeping `from` and `to` in sync, replacing an auto-sync that
    /// is running. Must be called within a Tokio runtime.
    pub fn start(
        self: &Arc<Self>,
        from: Location,
        to: Location,
        direction: LiveDirection,
    ) -> Result<LiveStatus> {
        if from == to {
            return Err(Error::Invalid(
                "both panes show the same folder; open another folder in one of them".into(),
            ));
        }
        self.stop();
        let changed = Arc::new(Notify::new());
        // Keep the watchers alive as long as the task runs.
        let from_watch = watch(&from, changed.clone());
        let to_watch = watch(&to, changed.clone());
        let status = Arc::new(Mutex::new(LiveStatus {
            from: from.clone(),
            to: to.clone(),
            direction,
            state: LiveState::Syncing,
            from_watched: from_watch.is_some(),
            to_watched: to_watch.is_some(),
            last_sync_ms: None,
            items_synced: 0,
            syncs: 0,
            error: None,
            conflicts: 0,
        }));
        let cancel = CancellationToken::new();
        *self.running.lock().unwrap() = Some(Running {
            cancel: cancel.clone(),
            status: status.clone(),
        });
        let first = status.lock().unwrap().clone();
        let _ = self.tx.send(Some(first.clone()));

        let this = self.clone();
        tokio::spawn(async move {
            let _watchers = (from_watch, to_watch);
            this.run(status, changed, cancel).await;
        });
        Ok(first)
    }

    /// Stop auto-sync. A sync step already copying is cancelled.
    pub fn stop(&self) {
        if let Some(r) = self.running.lock().unwrap().take() {
            r.cancel.cancel();
            let _ = self.tx.send(None);
        }
    }

    fn publish(
        &self,
        status: &Mutex<LiveStatus>,
        cancel: &CancellationToken,
        f: impl FnOnce(&mut LiveStatus),
    ) {
        if cancel.is_cancelled() {
            return; // stopped: the UI was already told
        }
        let mut s = status.lock().unwrap();
        f(&mut s);
        let _ = self.tx.send(Some(s.clone()));
    }

    async fn run(
        &self,
        status: Arc<Mutex<LiveStatus>>,
        changed: Arc<Notify>,
        cancel: CancellationToken,
    ) {
        let (from, to, direction, watched) = {
            let s = status.lock().unwrap();
            (
                s.from.clone(),
                s.to.clone(),
                s.direction,
                s.from_watched && s.to_watched,
            )
        };
        let interval = if watched {
            SAFETY_INTERVAL
        } else {
            POLL_INTERVAL
        };
        loop {
            self.publish(&status, &cancel, |s| s.state = LiveState::Syncing);
            let pass = self.sync_once(&from, &to, direction, &cancel).await;
            if cancel.is_cancelled() {
                return;
            }
            let wait = match pass {
                Ok(done) => {
                    self.publish(&status, &cancel, |s| {
                        s.state = LiveState::Watching;
                        s.error = None;
                        s.syncs += 1;
                        s.items_synced += done.items;
                        s.conflicts = done.conflicts;
                        s.last_sync_ms = Some(chrono::Utc::now().timestamp_millis());
                    });
                    interval
                }
                Err(e) => {
                    self.publish(&status, &cancel, |s| {
                        s.state = LiveState::Failed;
                        s.error = Some(e.to_string());
                    });
                    RETRY_AFTER.min(interval)
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = changed.notified() => {
                    // Let a burst of changes settle into one sync.
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(SETTLE) => {}
                    }
                }
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Compare once and copy what changed.
    async fn sync_once(
        &self,
        from: &Location,
        to: &Location,
        direction: LiveDirection,
        cancel: &CancellationToken,
    ) -> Result<Pass> {
        let mode = match direction {
            LiveDirection::OneWay => SyncMode::Update,
            LiveDirection::Both => SyncMode::TwoWay,
        };
        let preview = self
            .engine
            .plan(from, to, mode, options(), cancel.child_token(), None)
            .await?;
        let items = preview
            .steps
            .iter()
            .filter(|s| s.action != SyncAction::Skip)
            .count() as u64;
        let conflicts = preview
            .steps
            .iter()
            .filter(|s| s.action == SyncAction::Skip)
            .count() as u64;
        if items == 0 {
            return Ok(Pass { items, conflicts });
        }
        let run = self.engine.run(preview.id, &Default::default())?;
        let stop = {
            let queue = self.queue.clone();
            let op = run.op;
            let cancel = cancel.clone();
            tokio::spawn(async move {
                cancel.cancelled().await;
                queue.cancel(op);
            })
        };
        let info = self.queue.wait(run.op).await;
        let _ = run.done.await; // the snapshot for the next two-way sync
        stop.abort();
        match info {
            Some(i) if i.status == OperationStatus::Completed => Ok(Pass { items, conflicts }),
            Some(i) => Err(Error::Invalid(
                i.error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "the sync did not finish".into()),
            )),
            None => Err(Error::Invalid("the sync did not start".into())),
        }
    }
}

struct Pass {
    items: u64,
    conflicts: u64,
}

/// What auto-sync compares: like Compare & Sync, and never the temporary
/// files of copies in progress.
fn options() -> CompareOptions {
    let mut o = CompareOptions::default();
    o.exclude.push("*.dualpal-part".into());
    o.exclude.push(".dualpal-*".into());
    o
}

/// Watch a folder on this computer and its subfolders, signalling `changed`
/// on every change. `None` where the system sends no reliable change
/// notifications (servers, buckets, Windows network and WSL folders).
fn watch(loc: &Location, changed: Arc<Notify>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    if loc.fs != LocalFs::ID || loc.path.starts_with("\\\\") {
        return None;
    }
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res {
            if !e.kind.is_access() && !e.paths.iter().all(|p| is_temporary(p)) {
                changed.notify_one();
            }
        }
    })
    .ok()?;
    w.watch(std::path::Path::new(&loc.path), RecursiveMode::Recursive)
        .ok()?;
    Some(w)
}

/// Temporary files of copies in progress, which are not changes to sync.
fn is_temporary(p: &std::path::Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".dualpal-part") || n.starts_with(".dualpal-"))
}

//! Folder synchronization, one-way or two-way, between any two file systems.
//!
//! A sync starts from a [`Comparison`]. To tell "deleted here" from "new
//! there" and "changed here" from "changed there", the engine keeps a
//! snapshot per folder pair of what both sides looked like after the last
//! sync. Without one (the first sync), dates decide and nothing is deleted
//! in two-way mode.
//!
//! [`SyncEngine::plan`] returns the steps it would take; the UI may change
//! any of them before [`SyncEngine::run`] queues them as one operation.
//! When the operation ends (even half way), the snapshot is updated with
//! what was actually done.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::compare::{self, CompareOptions, Comparison, DiffItem, DiffState, ProgressFn, Side};
use crate::error::{Error, Result};
use crate::ops::{Operation, OperationQueue};
use crate::vfs::Location;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncMode {
    /// Copy new and changed items from left to right. Never deletes, and
    /// leaves items changed on the right alone.
    #[default]
    Update,
    /// Make the right side an exact copy of the left: copies, replaces and
    /// deletes on the right as needed.
    Mirror,
    /// Copy changes both ways, and delete what was deleted on the other side
    /// since the last sync.
    TwoWay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncAction {
    CopyToRight,
    CopyToLeft,
    DeleteLeft,
    DeleteRight,
    Skip,
}

/// Why a step was planned, for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncReason {
    /// Only on one side, and not there at the last sync.
    New,
    /// Changed on one side only.
    Changed,
    /// Deleted on the other side since the last sync.
    Deleted,
    /// Only on the right, which a mirror removes.
    Extra,
    /// Only on the right; Update mode leaves it.
    OnlyOnTarget,
    /// Changed on the right only; Update mode leaves it.
    TargetChanged,
    /// Changed on both sides.
    Conflict,
    /// Changed on one side and deleted on the other: it is copied back.
    ChangedAndDeleted,
    /// A file on one side, a folder on the other.
    KindMismatch,
}

/// Files and bytes an item holds on one side (a whole folder, for folders).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStep {
    pub rel: String,
    pub dir: bool,
    pub action: SyncAction,
    pub reason: SyncReason,
    /// Needs a look: the engine could not tell which side should win.
    pub conflict: bool,
    pub left: Option<Totals>,
    pub right: Option<Totals>,
}

impl SyncStep {
    /// Whether `action` can be carried out given what exists on each side.
    pub fn allows(&self, action: SyncAction) -> bool {
        match action {
            SyncAction::CopyToRight | SyncAction::DeleteLeft => self.left.is_some(),
            SyncAction::CopyToLeft | SyncAction::DeleteRight => self.right.is_some(),
            SyncAction::Skip => true,
        }
    }
}

/// Both sides of one path as they were after the last sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapPair {
    pub l: Side,
    pub r: Side,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub entries: BTreeMap<String, SnapPair>,
}

/// What a sync operation did to one path, reported by the executor.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncDone {
    /// Both sides now hold the same item.
    Copied { rel: String, pair: SnapPair },
    /// Gone from both sides.
    Deleted { rel: String },
}

fn under(rel: &str, dir: &str) -> bool {
    rel.len() > dir.len() && rel.starts_with(dir) && rel.as_bytes()[dir.len()] == b'/'
}

/// Work out the steps that take the two sides of `cmp` in sync.
pub fn plan_steps(cmp: &Comparison, snap: &Snapshot, mode: SyncMode) -> Vec<SyncStep> {
    let tol = cmp.options.tolerance_ms;
    let items = &cmp.items;
    let mut steps = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let item = &items[i];
        // Contents of a folder that exists on one side only follow it.
        let mut end = i + 1;
        while end < items.len() && under(&items[end].rel, &item.rel) {
            end += 1;
        }
        let inside = &items[i + 1..end];
        let next = match item.state {
            DiffState::LeftOnly | DiffState::RightOnly => end,
            _ => i + 1,
        };
        let totals = |pick: fn(&DiffItem) -> Option<Side>| -> Option<Totals> {
            let me = pick(item)?;
            let mut t = Totals {
                files: u64::from(!me.dir),
                bytes: me.size,
            };
            if me.dir {
                for c in inside.iter().filter_map(pick).filter(|s| !s.dir) {
                    t.files += 1;
                    t.bytes += c.size;
                }
            }
            Some(t)
        };
        let step = |action, reason, conflict| SyncStep {
            rel: item.rel.clone(),
            dir: item.dir,
            action,
            reason,
            conflict,
            left: totals(|d| d.left),
            right: totals(|d| d.right),
        };
        use SyncAction::*;
        use SyncReason as R;
        let planned = match item.state {
            DiffState::Same => None,
            DiffState::Different if item.dir => None,
            DiffState::KindMismatch => Some(step(Skip, R::KindMismatch, true)),
            DiffState::LeftOnly | DiffState::RightOnly => {
                let left = item.state == DiffState::LeftOnly;
                let side = |p: &SnapPair| if left { p.l } else { p.r };
                let cur = |d: &DiffItem| if left { d.left } else { d.right };
                let in_snap = snap.entries.contains_key(&item.rel);
                // Anything here that differs from (or is missing in) the snapshot.
                let changed = std::iter::once(item).chain(inside).any(|d| {
                    let Some(c) = cur(d) else { return false };
                    snap.entries
                        .get(&d.rel)
                        .is_none_or(|p| !side(p).matches(&c, tol))
                });
                let (copy, delete) = if left {
                    (CopyToRight, DeleteLeft)
                } else {
                    (CopyToLeft, DeleteRight)
                };
                Some(match (mode, left) {
                    (SyncMode::Update | SyncMode::Mirror, true) => step(CopyToRight, R::New, false),
                    (SyncMode::Update, false) => step(Skip, R::OnlyOnTarget, false),
                    (SyncMode::Mirror, false) => step(DeleteRight, R::Extra, false),
                    (SyncMode::TwoWay, _) if !in_snap => step(copy, R::New, false),
                    (SyncMode::TwoWay, _) if changed => step(copy, R::ChangedAndDeleted, true),
                    (SyncMode::TwoWay, _) => step(delete, R::Deleted, false),
                })
            }
            DiffState::LeftNewer | DiffState::RightNewer | DiffState::Different => {
                let (l, r) = (item.left.unwrap(), item.right.unwrap());
                let (lc, rc) = match snap.entries.get(&item.rel) {
                    Some(p) => (!p.l.matches(&l, tol), !p.r.matches(&r, tol)),
                    None => match item.state {
                        DiffState::LeftNewer => (true, false),
                        DiffState::RightNewer => (false, true),
                        _ => (true, true),
                    },
                };
                match (mode, lc, rc) {
                    // Unchanged on both sides since they were last in sync
                    // (e.g. a server that stamps uploads with its own time).
                    (_, false, false) => None,
                    (SyncMode::Mirror, _, _) => Some(step(CopyToRight, R::Changed, false)),
                    (_, true, false) => Some(step(CopyToRight, R::Changed, false)),
                    (SyncMode::TwoWay, false, true) => Some(step(CopyToLeft, R::Changed, false)),
                    (SyncMode::Update, false, true) => Some(step(Skip, R::TargetChanged, false)),
                    (_, true, true) => Some(step(Skip, R::Conflict, true)),
                }
            }
        };
        steps.extend(planned);
        i = next;
    }
    steps
}

/// The snapshot after a sync: the old one, minus paths gone from both
/// sides, plus everything that matched and everything the sync did.
pub fn next_snapshot(old: &Snapshot, cmp: &Comparison, done: &[SyncDone]) -> Snapshot {
    let present: std::collections::HashSet<&str> =
        cmp.items.iter().map(|i| i.rel.as_str()).collect();
    let mut entries: BTreeMap<String, SnapPair> = old
        .entries
        .iter()
        .filter(|(k, _)| present.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    for i in &cmp.items {
        let (Some(l), Some(r)) = (i.left, i.right) else {
            continue;
        };
        if i.state == DiffState::Same || (l.dir && r.dir) {
            entries.insert(i.rel.clone(), SnapPair { l, r });
        }
    }
    for d in done {
        match d {
            SyncDone::Copied { rel, pair } => {
                entries.insert(rel.clone(), *pair);
            }
            SyncDone::Deleted { rel } => {
                entries.retain(|k, _| k != rel && !under(k, rel));
            }
        }
    }
    Snapshot { entries }
}

/// The plan shown to the user before syncing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPreview {
    pub id: Uuid,
    pub mode: SyncMode,
    pub comparison: Comparison,
    pub steps: Vec<SyncStep>,
    /// A snapshot from an earlier sync of this pair was used.
    pub has_history: bool,
}

/// A saved folder pair with its sync settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncFavorite {
    pub id: String,
    pub name: String,
    pub left: Location,
    pub right: Location,
    #[serde(default)]
    pub mode: SyncMode,
    #[serde(default)]
    pub options: CompareOptions,
}

/// Saved sync favorites, persisted as JSON.
pub struct SyncFavorites {
    path: Option<PathBuf>,
    items: Mutex<Vec<SyncFavorite>>,
}

impl SyncFavorites {
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            items: Mutex::new(items),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            items: Mutex::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<SyncFavorite> {
        self.items.lock().unwrap().clone()
    }

    /// Add, or replace the favorite with the same id. An empty id gets a new one.
    pub fn save(&self, mut fav: SyncFavorite) -> Result<SyncFavorite> {
        if fav.name.trim().is_empty() {
            return Err(Error::Invalid("a sync favorite needs a name".into()));
        }
        if fav.id.is_empty() {
            fav.id = Uuid::new_v4().to_string();
        }
        let mut items = self.items.lock().unwrap();
        match items.iter_mut().find(|f| f.id == fav.id) {
            Some(f) => *f = fav.clone(),
            None => items.push(fav.clone()),
        }
        write_json(self.path.as_deref(), &*items)?;
        Ok(fav)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        items.retain(|f| f.id != id);
        write_json(self.path.as_deref(), &*items)
    }
}

fn write_json<T: Serialize + ?Sized>(path: Option<&Path>, value: &T) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(|e| Error::Invalid(e.to_string()))?;
    // Write a temporary file and rename, so a crash never leaves half a file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| Error::io(tmp.to_string_lossy(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::io(path.to_string_lossy(), e))
}

/// FNV-1a: a stable, dependency-free hash for file names.
pub fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

fn pair_key(left: &Location, right: &Location) -> String {
    format!(
        "{:016x}",
        fnv(&format!(
            "{}\n{}\n{}\n{}",
            left.fs, left.path, right.fs, right.path
        ))
    )
}

struct StoredPlan {
    comparison: Comparison,
    steps: Vec<SyncStep>,
    snapshot: Snapshot,
}

/// A started sync: the operation id, and a task that ends once the
/// snapshot has been saved.
pub struct SyncRun {
    pub op: Uuid,
    pub done: tokio::task::JoinHandle<()>,
}

pub struct SyncEngine {
    queue: OperationQueue,
    /// Snapshots are kept here, one JSON file per folder pair; in memory when None.
    state_dir: Option<PathBuf>,
    memory: Mutex<HashMap<String, Snapshot>>,
    plans: Mutex<HashMap<Uuid, StoredPlan>>,
}

impl SyncEngine {
    pub fn new(queue: OperationQueue, state_dir: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            queue,
            state_dir,
            memory: Mutex::new(HashMap::new()),
            plans: Mutex::new(HashMap::new()),
        })
    }

    fn snapshot_path(&self, key: &str) -> Option<PathBuf> {
        self.state_dir
            .as_ref()
            .map(|d| d.join(format!("{key}.json")))
    }

    pub fn load_snapshot(&self, left: &Location, right: &Location) -> Option<Snapshot> {
        let key = pair_key(left, right);
        match self.snapshot_path(&key) {
            Some(p) => std::fs::read(p)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok()),
            None => self.memory.lock().unwrap().get(&key).cloned(),
        }
    }

    fn save_snapshot(&self, left: &Location, right: &Location, snap: Snapshot) -> Result<()> {
        let key = pair_key(left, right);
        match self.snapshot_path(&key) {
            Some(p) => write_json(Some(&p), &snap),
            None => {
                self.memory.lock().unwrap().insert(key, snap);
                Ok(())
            }
        }
    }

    /// Forget what the last sync of this pair saw, so the next one starts fresh.
    pub fn forget(&self, left: &Location, right: &Location) {
        let key = pair_key(left, right);
        match self.snapshot_path(&key) {
            Some(p) => {
                let _ = std::fs::remove_file(p);
            }
            None => {
                self.memory.lock().unwrap().remove(&key);
            }
        }
    }

    /// Compare the folders and plan a sync in `mode`.
    pub async fn plan(
        &self,
        left: &Location,
        right: &Location,
        mode: SyncMode,
        options: CompareOptions,
        cancel: CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<SyncPreview> {
        let comparison = compare::compare(
            self.queue.registry(),
            left,
            right,
            options,
            cancel,
            progress,
        )
        .await?;
        let stored = self.load_snapshot(left, right);
        let has_history = stored.is_some();
        let snapshot = stored.unwrap_or_default();
        let steps = plan_steps(&comparison, &snapshot, mode);
        let id = Uuid::new_v4();
        let preview = SyncPreview {
            id,
            mode,
            comparison: comparison.clone(),
            steps: steps.clone(),
            has_history,
        };
        let mut plans = self.plans.lock().unwrap();
        // Only the newest few plans can still be run.
        if plans.len() > 8 {
            plans.clear();
        }
        plans.insert(
            id,
            StoredPlan {
                comparison,
                steps,
                snapshot,
            },
        );
        Ok(preview)
    }

    /// Queue the planned steps, with the user's changes in `overrides`
    /// (relative path to action). Must be called within a Tokio runtime.
    pub fn run(
        self: &Arc<Self>,
        plan: Uuid,
        overrides: &HashMap<String, SyncAction>,
    ) -> Result<SyncRun> {
        let stored = self.plans.lock().unwrap().remove(&plan).ok_or_else(|| {
            Error::Invalid("this comparison is out of date; compare again".into())
        })?;
        let mut steps = stored.steps.clone();
        for s in &mut steps {
            if let Some(&a) = overrides.get(&s.rel) {
                if !s.allows(a) {
                    return Err(Error::Invalid(format!("cannot do that with {}", s.rel)));
                }
                s.action = a;
            }
        }
        steps.retain(|s| s.action != SyncAction::Skip);
        let left = stored.comparison.left.clone();
        let right = stored.comparison.right.clone();
        let op = self.queue.enqueue(Operation::Sync {
            left: left.clone(),
            right: right.clone(),
            steps,
        });
        let engine = self.clone();
        let done = tokio::spawn(async move {
            engine.queue.wait(op).await;
            let results = engine.queue.take_sync_results(op);
            let next = next_snapshot(&stored.snapshot, &stored.comparison, &results);
            if let Err(e) = engine.save_snapshot(&left, &right, next) {
                eprintln!("could not save the sync snapshot: {e}");
            }
        });
        Ok(SyncRun { op, done })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{CompareSummary, DiffItem};

    fn f(size: u64, t: i64) -> Option<Side> {
        Some(Side {
            dir: false,
            size,
            modified_ms: Some(t),
        })
    }
    const D: Option<Side> = Some(Side {
        dir: true,
        size: 0,
        modified_ms: None,
    });

    fn item(rel: &str, left: Option<Side>, right: Option<Side>, state: DiffState) -> DiffItem {
        DiffItem {
            rel: rel.into(),
            name: rel.rsplit('/').next().unwrap().into(),
            depth: rel.matches('/').count() as u32,
            dir: left.or(right).unwrap().dir,
            left,
            right,
            state,
        }
    }

    fn cmp(items: Vec<DiffItem>) -> Comparison {
        Comparison {
            left: Location::new("a", "/l"),
            right: Location::new("b", "/r"),
            options: CompareOptions::default(),
            items,
            summary: CompareSummary::default(),
        }
    }

    fn actions(steps: &[SyncStep]) -> Vec<(&str, SyncAction, bool)> {
        steps
            .iter()
            .map(|s| (s.rel.as_str(), s.action, s.conflict))
            .collect()
    }

    #[test]
    fn first_sync_uses_dates_and_never_deletes() {
        let c = cmp(vec![
            item("a.txt", f(1, 10_000), None, DiffState::LeftOnly),
            item("b.txt", None, f(1, 10_000), DiffState::RightOnly),
            item("c.txt", f(1, 90_000), f(2, 10_000), DiffState::LeftNewer),
            item("d.txt", f(1, 10_000), f(2, 90_000), DiffState::RightNewer),
            item("e.txt", f(1, 10_000), f(2, 10_000), DiffState::Different),
        ]);
        let snap = Snapshot::default();
        use SyncAction::*;
        assert_eq!(
            actions(&plan_steps(&c, &snap, SyncMode::TwoWay)),
            vec![
                ("a.txt", CopyToRight, false),
                ("b.txt", CopyToLeft, false),
                ("c.txt", CopyToRight, false),
                ("d.txt", CopyToLeft, false),
                ("e.txt", Skip, true),
            ]
        );
        assert_eq!(
            actions(&plan_steps(&c, &snap, SyncMode::Update)),
            vec![
                ("a.txt", CopyToRight, false),
                ("b.txt", Skip, false),
                ("c.txt", CopyToRight, false),
                ("d.txt", Skip, false),
                ("e.txt", Skip, true),
            ]
        );
        assert_eq!(
            actions(&plan_steps(&c, &snap, SyncMode::Mirror)),
            vec![
                ("a.txt", CopyToRight, false),
                ("b.txt", DeleteRight, false),
                ("c.txt", CopyToRight, false),
                ("d.txt", CopyToRight, false),
                ("e.txt", CopyToRight, false),
            ]
        );
    }

    #[test]
    fn snapshot_tells_deleted_from_new_and_who_changed() {
        let synced = |l, r| SnapPair {
            l: f(1, l).unwrap(),
            r: f(1, r).unwrap(),
        };
        let mut snap = Snapshot::default();
        // Was on both sides; now only on the left: deleted on the right.
        snap.entries
            .insert("gone.txt".into(), synced(10_000, 50_000));
        // Was on both sides; changed on the left, deleted on the right.
        snap.entries
            .insert("kept.txt".into(), synced(10_000, 50_000));
        // The server stamped its copy later: unchanged on both sides.
        snap.entries
            .insert("same.txt".into(), synced(10_000, 50_000));
        // Edited on the right although the left looks newer by date.
        snap.entries
            .insert("edited.txt".into(), synced(10_000, 1_000));
        // Edited on both sides.
        snap.entries
            .insert("both.txt".into(), synced(10_000, 10_000));
        snap.entries.insert(
            "dir".into(),
            SnapPair {
                l: D.unwrap(),
                r: D.unwrap(),
            },
        );
        snap.entries.insert("dir/x".into(), synced(10_000, 10_000));
        let c = cmp(vec![
            item("dir", None, D, DiffState::RightOnly),
            item("dir/x", None, f(1, 10_000), DiffState::RightOnly),
            item(
                "both.txt",
                f(2, 20_000),
                f(3, 30_000),
                DiffState::RightNewer,
            ),
            item(
                "edited.txt",
                f(1, 10_000),
                f(5, 2_000),
                DiffState::LeftNewer,
            ),
            item("gone.txt", f(1, 10_000), None, DiffState::LeftOnly),
            item("kept.txt", f(9, 70_000), None, DiffState::LeftOnly),
            item(
                "same.txt",
                f(1, 10_000),
                f(1, 50_000),
                DiffState::RightNewer,
            ),
        ]);
        use SyncAction::*;
        assert_eq!(
            actions(&plan_steps(&c, &snap, SyncMode::TwoWay)),
            vec![
                ("dir", DeleteRight, false),
                ("both.txt", Skip, true),
                ("edited.txt", CopyToLeft, false),
                ("gone.txt", DeleteLeft, false),
                ("kept.txt", CopyToRight, true),
            ]
        );
        // Update copies the left's changes and leaves the right's alone.
        assert_eq!(
            actions(&plan_steps(&c, &snap, SyncMode::Update)),
            vec![
                ("dir", Skip, false),
                ("both.txt", Skip, true),
                ("edited.txt", Skip, false),
                ("gone.txt", CopyToRight, false),
                ("kept.txt", CopyToRight, false),
            ]
        );
    }

    #[test]
    fn folders_on_one_side_are_one_step_with_totals() {
        let c = cmp(vec![
            item("new", D, None, DiffState::LeftOnly),
            item("new/a", f(5, 1), None, DiffState::LeftOnly),
            item("new/sub", D, None, DiffState::LeftOnly),
            item("new/sub/b", f(7, 1), None, DiffState::LeftOnly),
            item("z", f(1, 1), None, DiffState::LeftOnly),
        ]);
        let steps = plan_steps(&c, &Snapshot::default(), SyncMode::TwoWay);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].rel, "new");
        assert_eq!(
            steps[0].left,
            Some(Totals {
                files: 2,
                bytes: 12
            })
        );
        assert_eq!(steps[0].right, None);
        assert!(!steps[0].allows(SyncAction::CopyToLeft));
    }

    #[test]
    fn next_snapshot_records_matches_and_results() {
        let mut old = Snapshot::default();
        let p = SnapPair {
            l: f(1, 1).unwrap(),
            r: f(1, 1).unwrap(),
        };
        old.entries.insert("vanished".into(), p);
        old.entries.insert("conflict".into(), p);
        old.entries.insert("dir/sub".into(), p);
        let c = cmp(vec![
            item("dir", D, D, DiffState::Different),
            item("dir/sub", f(1, 1), None, DiffState::LeftOnly),
            item("conflict", f(2, 5), f(3, 5), DiffState::Different),
            item("same", f(4, 4), f(4, 4), DiffState::Same),
            item("copied", f(6, 6), None, DiffState::LeftOnly),
        ]);
        let pc = SnapPair {
            l: f(6, 6).unwrap(),
            r: f(6, 99).unwrap(),
        };
        let next = next_snapshot(
            &old,
            &c,
            &[
                SyncDone::Copied {
                    rel: "copied".into(),
                    pair: pc,
                },
                SyncDone::Deleted { rel: "dir".into() },
            ],
        );
        let keys: Vec<&str> = next.entries.keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["conflict", "copied", "same"]);
        // A conflict keeps its old entry, so it is still seen as one next time.
        assert_eq!(next.entries["conflict"], p);
        assert_eq!(next.entries["copied"], pc);
    }
}

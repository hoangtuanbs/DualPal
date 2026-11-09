//! Sync planning and favorites cases (Phase 1, §8.8).

use std::collections::HashMap;
use std::sync::Arc;

use dualpal_core::compare::{
    CompareOptions, CompareSummary, Comparison, DiffItem, DiffState, Side,
};
use dualpal_core::sync::{
    fnv, next_snapshot, plan_steps, SnapPair, Snapshot, SyncAction, SyncEngine, SyncFavorite,
    SyncFavorites, SyncMode, SyncReason, SyncStep, Totals,
};
use dualpal_core::{ActivityLog, FileSystem, Location, OperationQueue, OperationStatus, Registry};
use dualpal_testkit::MemFs;
use tokio_util::sync::CancellationToken;

fn side(ms: i64) -> Side {
    Side {
        dir: false,
        size: 5,
        modified_ms: Some(ms),
    }
}

fn file_item(state: DiffState, l: Option<Side>, r: Option<Side>) -> DiffItem {
    DiffItem {
        rel: "f.txt".into(),
        name: "f.txt".into(),
        depth: 0,
        dir: false,
        left: l,
        right: r,
        state,
    }
}

fn comparison(items: Vec<DiffItem>) -> Comparison {
    Comparison {
        left: Location::new("a", "/l"),
        right: Location::new("b", "/r"),
        options: CompareOptions::default(),
        items,
        summary: CompareSummary::default(),
    }
}

fn snapshot(pairs: &[(&str, (i64, i64))]) -> Snapshot {
    Snapshot {
        entries: pairs
            .iter()
            .map(|(rel, (l, r))| {
                (
                    rel.to_string(),
                    SnapPair {
                        l: side(*l),
                        r: side(*r),
                    },
                )
            })
            .collect(),
    }
}

fn planned(items: Vec<DiffItem>, snap: &Snapshot, mode: SyncMode) -> Vec<SyncStep> {
    plan_steps(&comparison(items), snap, mode)
}

/// SYNC-001 — the plan matrix: mode × state → action, reason, conflict.
///
/// This table is the spec for how a sync behaves.
#[test]
fn plan_matrix() {
    use DiffState::*;
    use SyncAction as A;
    use SyncReason as R;

    let none = Snapshot::default();
    // A snapshot in which `f.txt` matched at mtime 1000 on both sides.
    let in_sync = snapshot(&[("f.txt", (1000, 1000))]);

    let both = (Some(side(2000)), Some(side(2000)));
    let left_newer = (Some(side(2000)), Some(side(1000)));
    let right_newer = (Some(side(1000)), Some(side(2000)));

    // Update (→ right): never deletes, leaves right-only and right-changed alone.
    for (state, l, r, want) in [
        (
            LeftOnly,
            Some(side(1000)),
            None,
            Some((A::CopyToRight, R::New, false)),
        ),
        (
            RightOnly,
            None,
            Some(side(1000)),
            Some((A::Skip, R::OnlyOnTarget, false)),
        ),
        (
            LeftNewer,
            left_newer.0,
            left_newer.1,
            Some((A::CopyToRight, R::Changed, false)),
        ),
        (
            RightNewer,
            right_newer.0,
            right_newer.1,
            Some((A::Skip, R::TargetChanged, false)),
        ),
        (
            Different,
            both.0,
            both.1,
            Some((A::Skip, R::Conflict, true)),
        ),
        (
            KindMismatch,
            Some(side(0)),
            Some(side(0)),
            Some((A::Skip, R::KindMismatch, true)),
        ),
        (Same, both.0, both.1, None),
    ] {
        let steps = planned(vec![file_item(state, l, r)], &none, SyncMode::Update);
        match want {
            Some((a, r2, c)) => {
                assert_eq!(steps.len(), 1, "{state:?}");
                assert_eq!(
                    (steps[0].action, steps[0].reason, steps[0].conflict),
                    (a, r2, c)
                );
            }
            None => assert!(steps.is_empty(), "{state:?}"),
        }
    }

    // Mirror (right becomes an exact copy of left).
    for (state, l, r, want) in [
        (
            LeftOnly,
            Some(side(1000)),
            None,
            Some((A::CopyToRight, R::New, false)),
        ),
        (
            RightOnly,
            None,
            Some(side(1000)),
            Some((A::DeleteRight, R::Extra, false)),
        ),
        (
            RightNewer,
            right_newer.0,
            right_newer.1,
            Some((A::CopyToRight, R::Changed, false)),
        ),
        (
            Different,
            both.0,
            both.1,
            Some((A::CopyToRight, R::Changed, false)),
        ),
    ] {
        let steps = planned(vec![file_item(state, l, r)], &none, SyncMode::Mirror);
        let Some((a, r2, c)) = want else { continue };
        assert_eq!(steps.len(), 1, "{state:?}");
        assert_eq!(
            (steps[0].action, steps[0].reason, steps[0].conflict),
            (a, r2, c)
        );
    }

    // Two-way: first sync never deletes; a later deletion is passed on.
    let steps = planned(
        vec![file_item(LeftOnly, Some(side(1000)), None)],
        &none,
        SyncMode::TwoWay,
    );
    assert_eq!(steps[0].action, A::CopyToRight);
    let steps = planned(
        vec![file_item(RightOnly, None, Some(side(1000)))],
        &none,
        SyncMode::TwoWay,
    );
    assert_eq!(steps[0].action, A::CopyToLeft);

    // LeftOnly still matches the snapshot → it was deleted on the right.
    let steps = planned(
        vec![file_item(LeftOnly, Some(side(1000)), None)],
        &in_sync,
        SyncMode::TwoWay,
    );
    assert_eq!(
        (steps[0].action, steps[0].reason),
        (A::DeleteLeft, R::Deleted)
    );

    // RightOnly still matches the snapshot → deleted on the left.
    let steps = planned(
        vec![file_item(RightOnly, None, Some(side(1000)))],
        &in_sync,
        SyncMode::TwoWay,
    );
    assert_eq!(
        (steps[0].action, steps[0].reason),
        (A::DeleteRight, R::Deleted)
    );

    // LeftOnly changed from the snapshot → copy it back (changed and deleted).
    let steps = planned(
        vec![file_item(LeftOnly, Some(side(5000)), None)],
        &in_sync,
        SyncMode::TwoWay,
    );
    assert_eq!(
        (steps[0].action, steps[0].reason, steps[0].conflict),
        (A::CopyToRight, R::ChangedAndDeleted, true)
    );
}

/// SYNC-005 — a file changed on both sides is left alone and reported.
#[tokio::test]
async fn conflict_is_skipped_and_untouched() {
    let registry = Arc::new(Registry::with_local());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(50)), 2);
    let engine = SyncEngine::new(q.clone(), None);
    let l = tempfile::tempdir().unwrap();
    let r = tempfile::tempdir().unwrap();
    let (ll, rl) = (
        Location::new("local", l.path().to_string_lossy()),
        Location::new("local", r.path().to_string_lossy()),
    );

    std::fs::write(l.path().join("both.txt"), "same").unwrap();
    std::fs::write(r.path().join("both.txt"), "same").unwrap();

    // First sync records that both.txt matched.
    let plan = engine
        .plan(
            &ll,
            &rl,
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(plan.steps.is_empty());
    engine.settle(plan.id).unwrap();

    // Both sides change it now.
    std::fs::write(l.path().join("both.txt"), "left version").unwrap();
    std::fs::write(r.path().join("both.txt"), "right v").unwrap();

    let plan = engine
        .plan(
            &ll,
            &rl,
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let step = plan.steps.iter().find(|s| s.rel == "both.txt").unwrap();
    assert_eq!(step.action, SyncAction::Skip);
    assert!(step.conflict);

    // Running the plan leaves both files exactly as they were.
    let run = engine.run(plan.id, &HashMap::new()).unwrap();
    run.done.await.unwrap();
    assert_eq!(q.get(run.op).unwrap().status, OperationStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(l.path().join("both.txt")).unwrap(),
        "left version"
    );
    assert_eq!(
        std::fs::read_to_string(r.path().join("both.txt")).unwrap(),
        "right v"
    );
}

/// SYNC-006 — an override is rejected when the item is not on that side.
#[test]
fn overrides_are_checked_against_what_exists() {
    let step = SyncStep {
        rel: "f.txt".into(),
        dir: false,
        action: SyncAction::CopyToRight,
        reason: SyncReason::Changed,
        conflict: false,
        left: Some(Totals { files: 1, bytes: 5 }),
        right: None,
    };
    assert!(step.allows(SyncAction::CopyToRight));
    assert!(step.allows(SyncAction::DeleteLeft));
    assert!(step.allows(SyncAction::Skip));
    assert!(!step.allows(SyncAction::CopyToLeft), "not on the right");
    assert!(!step.allows(SyncAction::DeleteRight), "not on the right");
}

/// SYNC-012 — snapshot state is keyed per pair, so pairs are independent.
#[tokio::test]
async fn snapshot_is_per_pair() {
    // fnv is deterministic and cheap.
    assert_eq!(fnv("x"), fnv("x"));
    assert_ne!(fnv("/a"), fnv("/b"));

    let registry = Arc::new(Registry::with_local());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(50)), 2);
    let state = tempfile::tempdir().unwrap();
    let engine = SyncEngine::new(q.clone(), Some(state.path().to_path_buf()));

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let c = tempfile::tempdir().unwrap();
    std::fs::write(a.path().join("x.txt"), "x").unwrap();
    std::fs::write(b.path().join("x.txt"), "x").unwrap();
    let (la, lb, lc) = (
        Location::new("local", a.path().to_string_lossy()),
        Location::new("local", b.path().to_string_lossy()),
        Location::new("local", c.path().to_string_lossy()),
    );

    // Settle pair (a, b) so it has history.
    let plan = engine
        .plan(
            &la,
            &lb,
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    engine.settle(plan.id).unwrap();

    // A different pair (a, c) has no history of its own.
    let plan = engine
        .plan(
            &la,
            &lc,
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(
        !plan.has_history,
        "a different pair must not inherit history"
    );
}

/// SYNC-013 — sync favorites save, list, replace and remove, and persist.
#[test]
fn sync_favorites_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("syncs.json");
    let f = SyncFavorites::open(&path);
    let fav = SyncFavorite {
        id: String::new(),
        name: "Backup".into(),
        left: Location::new("local", "/a"),
        right: Location::new("conn:x", "/backup"),
        mode: SyncMode::Mirror,
        options: CompareOptions::default(),
    };
    let saved = f.save(fav.clone()).unwrap();
    assert!(!saved.id.is_empty());
    assert_eq!(f.list().len(), 1);

    // Saving with the same id replaces.
    let mut changed = fav;
    changed.id = saved.id.clone();
    changed.name = "Backup 2".into();
    f.save(changed).unwrap();
    assert_eq!(f.list().len(), 1);
    assert_eq!(f.list()[0].name, "Backup 2");

    // Reopening reads it back.
    let reopened = SyncFavorites::open(&path);
    assert_eq!(reopened.list().len(), 1);
    reopened.remove(&saved.id).unwrap();
    assert!(SyncFavorites::open(&path).list().is_empty());
}

/// SYNC-009 — `next_snapshot` records matches and drops deletions.
#[test]
fn next_snapshot_records_matches_and_deletions() {
    let old = snapshot(&[("gone.txt", (1, 1)), ("kept.txt", (2, 2))]);
    // "gone.txt" is no longer present; "kept.txt" still is and matches.
    let cmp = comparison(vec![DiffItem {
        rel: "kept.txt".into(),
        name: "kept.txt".into(),
        depth: 0,
        dir: false,
        left: Some(side(2)),
        right: Some(side(2)),
        state: DiffState::Same,
    }]);
    let next = next_snapshot(&old, &cmp, &[]);
    // "gone.txt" is no longer present on either side, so it is dropped.
    assert!(!next.entries.contains_key("gone.txt"));
    // "kept.txt" is still present and matches, so it is kept.
    assert!(next.entries.contains_key("kept.txt"));
}

/// SYNC-017 — after a two-way sync of two disjoint sets, both sides are equal.
#[test]
fn two_way_sync_makes_both_sides_equal() {
    // Names differ in case only per side, so the sets are always disjoint on
    // the case-sensitive memory backend and can never conflict.
    proptest::proptest!(|(
        left in proptest::collection::vec(("[a-z][a-z0-9]{0,5}", "[a-z0-9]{0,8}"), 0..6),
        right in proptest::collection::vec(("[A-Z][A-Z0-9]{0,5}", "[A-Z0-9]{0,8}"), 0..6),
    )| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            run_two_way_case(left, right).await;
        });
    });
}

async fn run_two_way_case(left: Vec<(String, String)>, right: Vec<(String, String)>) {
    // The generator may produce duplicate names; keep the last content.
    let mut left_map: HashMap<String, String> = HashMap::new();
    for (n, c) in left {
        left_map.insert(n, c);
    }
    let mut right_map: HashMap<String, String> = HashMap::new();
    for (n, c) in right {
        right_map.insert(n, c);
    }

    let a_mem = MemFs::new("a");
    let b_mem = MemFs::new("b");
    for (n, c) in &left_map {
        a_mem.seed_file(&format!("/{n}.txt"), c.as_bytes());
    }
    for (n, c) in &right_map {
        b_mem.seed_file(&format!("/{n}.txt"), c.as_bytes());
    }
    let a: Arc<dyn FileSystem> = Arc::new(a_mem.clone());
    let b: Arc<dyn FileSystem> = Arc::new(b_mem.clone());
    let registry = Arc::new(Registry::with_local());
    registry.register(a.clone());
    registry.register(b.clone());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(100)), 2);
    let engine = SyncEngine::new(q.clone(), None);

    let plan = engine
        .plan(
            &Location::new("a", "/"),
            &Location::new("b", "/"),
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    // Disjoint names: a first sync must plan no deletions and no conflicts.
    assert!(plan.steps.iter().all(|s| !s.conflict), "{:?}", plan.steps);
    assert!(
        plan.steps
            .iter()
            .all(|s| s.action != SyncAction::DeleteLeft && s.action != SyncAction::DeleteRight),
        "{:?}",
        plan.steps
    );

    if !plan.steps.is_empty() {
        let run = engine.run(plan.id, &HashMap::new()).unwrap();
        run.done.await.unwrap();
        assert_eq!(q.get(run.op).unwrap().status, OperationStatus::Completed);
    }

    // Both sides now hold every file with the same content.
    for (n, c) in left_map.iter().chain(right_map.iter()) {
        let path = format!("/{n}.txt");
        let want = c.as_bytes();
        assert_eq!(
            a_mem.contents(&path).as_deref(),
            Some(want),
            "left lacks {path}"
        );
        assert_eq!(
            b_mem.contents(&path).as_deref(),
            Some(want),
            "right lacks {path}"
        );
    }
}

//! Sync engine cases from §8.8 still missing after Phase 1: failed steps,
//! forgetting history, single-use plans and step totals.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use dualpal_core::compare::CompareOptions;
use dualpal_core::sync::{SyncAction, SyncEngine, SyncMode, SyncPreview, SyncReason, Totals};
use dualpal_core::vfs::LocalFs;
use dualpal_core::{ActivityLog, Error, Location, OperationQueue, OperationStatus, Registry};
use dualpal_testkit::{FaultOp, FaultyFs};
use tokio_util::sync::CancellationToken;

fn local(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn engine_over(registry: Registry) -> (Arc<SyncEngine>, OperationQueue) {
    let q = OperationQueue::new(Arc::new(registry), Arc::new(ActivityLog::in_memory(100)), 2);
    (SyncEngine::new(q.clone(), None), q)
}

async fn plan(engine: &SyncEngine, l: &Location, r: &Location, mode: SyncMode) -> SyncPreview {
    engine
        .plan(
            l,
            r,
            mode,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap()
}

/// Run a plan as it stands, wait until its snapshot is saved, and return
/// the finished operation.
async fn run(
    engine: &Arc<SyncEngine>,
    q: &OperationQueue,
    p: &SyncPreview,
) -> dualpal_core::OperationInfo {
    let started = engine.run(p.id, &HashMap::new()).unwrap();
    started.done.await.unwrap();
    q.get(started.op).expect("operation known")
}

fn step_for<'a>(p: &'a SyncPreview, rel: &str) -> Option<&'a dualpal_core::sync::SyncStep> {
    p.steps.iter().find(|s| s.rel == rel)
}

/// SYNC-010 — a step that failed is not recorded as done: the next two-way
/// plan proposes the copy again instead of treating the file as deleted on
/// the other side.
#[tokio::test]
async fn failed_step_is_retried_next_time() {
    let tmp = tempfile::tempdir().unwrap();
    let left = tmp.path().join("left");
    let right = tmp.path().join("right");
    write(&left.join("a.txt"), "alpha");
    write(&left.join("b.txt"), "bravo");
    write(&left.join("c.txt"), "charlie");
    std::fs::create_dir(&right).unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    let registry = Registry::with_local();
    registry.register(remote.clone());
    let q = OperationQueue::new(Arc::new(registry), Arc::new(ActivityLog::in_memory(100)), 2);
    let engine = SyncEngine::new(q.clone(), None);
    let (l, r) = (
        local(&left),
        Location::new("remote", right.to_string_lossy()),
    );

    let first = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    assert_eq!(first.steps.len(), 3);
    // One upload fails for good (not a dropped connection, so no retry).
    remote.fail_next(
        FaultOp::OpenWrite,
        Error::PermissionDenied("quota exceeded".into()),
    );
    let started = engine.run(first.id, &HashMap::new()).unwrap();
    started.done.await.unwrap();
    let info = q.get(started.op).unwrap();
    assert_eq!(info.status, OperationStatus::Failed, "{:?}", info.error);

    let second = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    let mut retried = 0;
    for name in ["a.txt", "b.txt", "c.txt"] {
        let on_right = right.join(name).exists();
        match step_for(&second, name) {
            None => assert!(on_right, "{name}: no step, so it must have been copied"),
            Some(s) => {
                assert!(!on_right, "{name} was copied but is planned again");
                assert_eq!(s.action, SyncAction::CopyToRight, "{name}: {s:?}");
                assert_ne!(s.reason, SyncReason::Deleted, "{name}: {s:?}");
                retried += 1;
            }
        }
    }
    assert!(retried >= 1, "the failed file is proposed again");
    assert!(
        second
            .steps
            .iter()
            .all(|s| s.action != SyncAction::DeleteLeft),
        "nothing is deleted on the left: {:?}",
        second.steps
    );

    // The retry goes through and the pair is then in sync.
    let started = engine.run(second.id, &HashMap::new()).unwrap();
    started.done.await.unwrap();
    assert_eq!(
        q.get(started.op).unwrap().status,
        OperationStatus::Completed
    );
    let third = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    assert!(third.steps.is_empty(), "{:?}", third.steps);
}

/// SYNC-011 — after forgetting the history, the next two-way sync behaves
/// like a first one: it copies instead of deleting.
#[tokio::test]
async fn forget_history_makes_the_next_sync_a_first_one() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write(&a.path().join("keep.txt"), "keep");
    let (engine, q) = engine_over(Registry::with_local());
    let (l, r) = (local(a.path()), local(b.path()));

    let first = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    assert!(!first.has_history);
    assert_eq!(
        run(&engine, &q, &first).await.status,
        OperationStatus::Completed
    );
    assert!(b.path().join("keep.txt").exists());

    // Deleted on the right since the last sync: with history, a two-way
    // sync deletes it on the left too.
    std::fs::remove_file(b.path().join("keep.txt")).unwrap();
    let with_history = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    assert!(with_history.has_history);
    let s = step_for(&with_history, "keep.txt").unwrap();
    assert_eq!(
        (s.action, s.reason),
        (SyncAction::DeleteLeft, SyncReason::Deleted)
    );
    // The plan is not run; forget instead.
    engine.settle(with_history.id).ok();

    engine.forget(&l, &r);
    assert!(engine.load_snapshot(&l, &r).is_none());
    let fresh = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    assert!(!fresh.has_history);
    let s = step_for(&fresh, "keep.txt").unwrap();
    assert_eq!(
        (s.action, s.reason),
        (SyncAction::CopyToRight, SyncReason::New)
    );
}

/// SYNC-014 — a plan can be used once: running it again, or running it
/// after `settle`, is an `invalid` "out of date" error.
#[tokio::test]
async fn a_plan_is_consumed_once() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write(&a.path().join("x.txt"), "x");
    let (engine, _q) = engine_over(Registry::with_local());
    let (l, r) = (local(a.path()), local(b.path()));

    let p = plan(&engine, &l, &r, SyncMode::Update).await;
    let started = engine.run(p.id, &HashMap::new()).unwrap();
    started.done.await.unwrap();
    match engine.run(p.id, &HashMap::new()) {
        Err(e) => assert_eq!(e.kind(), "invalid", "{e}"),
        Ok(_) => panic!("a plan ran twice"),
    }

    let settled = plan(&engine, &l, &r, SyncMode::Update).await;
    engine.settle(settled.id).unwrap();
    assert!(engine.run(settled.id, &HashMap::new()).is_err());
    // Settling an unknown or used plan is harmless.
    engine.settle(settled.id).unwrap();
    engine.settle(uuid::Uuid::new_v4()).unwrap();

    // An override that the step cannot do is refused before anything runs.
    write(&a.path().join("y.txt"), "y");
    let p = plan(&engine, &l, &r, SyncMode::Update).await;
    let overrides = HashMap::from([("y.txt".to_string(), SyncAction::CopyToLeft)]);
    assert!(engine.run(p.id, &overrides).is_err());
    assert!(!b.path().join("y.txt").exists());
}

/// SYNC-015 — each step carries the files and bytes on each side; a folder
/// that exists on one side counts everything inside it.
#[tokio::test]
async fn step_totals_count_files_and_bytes_per_side() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write(&a.path().join("one.txt"), "12345"); // 5 bytes
    write(&a.path().join("folder/x.txt"), "abc"); // 3
    write(&a.path().join("folder/deep/y.txt"), "abcd"); // 4
    std::fs::create_dir_all(a.path().join("folder/empty")).unwrap();
    write(&b.path().join("only-right.txt"), "123456"); // 6
    write(&b.path().join("rfolder/z.txt"), "zz"); // 2
    let (engine, q) = engine_over(Registry::with_local());
    let (l, r) = (local(a.path()), local(b.path()));

    let p = plan(&engine, &l, &r, SyncMode::TwoWay).await;
    let get = |rel: &str| step_for(&p, rel).unwrap_or_else(|| panic!("{rel}: {:?}", p.steps));

    let one = get("one.txt");
    assert_eq!(one.action, SyncAction::CopyToRight);
    assert_eq!(one.left, Some(Totals { files: 1, bytes: 5 }));
    assert_eq!(one.right, None);

    let folder = get("folder");
    assert!(folder.dir);
    assert_eq!(folder.action, SyncAction::CopyToRight);
    assert_eq!(folder.left, Some(Totals { files: 2, bytes: 7 }));
    assert_eq!(folder.right, None);
    // Its contents are carried by the folder step, not listed again.
    assert!(step_for(&p, "folder/x.txt").is_none());

    let right_only = get("only-right.txt");
    assert_eq!(right_only.action, SyncAction::CopyToLeft);
    assert_eq!(right_only.right, Some(Totals { files: 1, bytes: 6 }));
    assert_eq!(right_only.left, None);

    let rfolder = get("rfolder");
    assert_eq!(rfolder.right, Some(Totals { files: 1, bytes: 2 }));

    // The operation's totals are the sum of what the copy steps move.
    let info = run(&engine, &q, &p).await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_total, 5);
    assert_eq!(info.progress.bytes_total, 5 + 7 + 6 + 2);
    assert_eq!(info.progress.bytes_done, info.progress.bytes_total);
    assert_eq!(
        std::fs::read_to_string(b.path().join("folder/deep/y.txt")).unwrap(),
        "abcd"
    );
    assert_eq!(
        std::fs::read_to_string(a.path().join("rfolder/z.txt")).unwrap(),
        "zz"
    );
}

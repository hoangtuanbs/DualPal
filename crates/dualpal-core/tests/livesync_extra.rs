//! Additional auto-sync cases (Phase 1, §8.9).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dualpal_core::livesync::{LiveDirection, LiveState, LiveSync};
use dualpal_core::sync::SyncEngine;
use dualpal_core::{ActivityLog, FileSystem, Location, OperationQueue, Registry};
use dualpal_testkit::{FaultyFs, MemFs};

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn setup() -> Arc<LiveSync> {
    let queue = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(100)),
        2,
    );
    LiveSync::new(SyncEngine::new(queue.clone(), None), queue)
}

async fn eventually(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

/// LIVE-006 — one-way auto-sync never deletes on the target.
#[tokio::test]
async fn one_way_never_deletes_on_target() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("keep.txt"), "x").unwrap();
    std::fs::copy(a.join("keep.txt"), b.join("keep.txt")).unwrap();
    let t = std::fs::metadata(a.join("keep.txt"))
        .unwrap()
        .modified()
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(b.join("keep.txt"))
        .unwrap()
        .set_modified(t)
        .unwrap();

    let live = setup();
    live.start(loc(&a), loc(&b), LiveDirection::OneWay)
        .await
        .unwrap();
    eventually("first sync done", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= 1)
    })
    .await;

    std::fs::remove_file(a.join("keep.txt")).unwrap();
    // The deletion triggers another pass; it must not remove the target file.
    eventually("the deletion was seen", || {
        live.status().is_some_and(|s| s.syncs >= 2)
    })
    .await;
    assert!(
        b.join("keep.txt").exists(),
        "one-way sync must not delete on the target"
    );
    live.stop().await;
}

/// LIVE-007 — one folder inside the other must be refused.
///
/// The current implementation only refuses two *identical* folders.
#[tokio::test]
#[ignore = "BUG: LiveSync only refuses identical folders, not one inside the other"]
async fn one_folder_inside_the_other_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    let live = setup();
    assert!(live
        .start(
            loc(&tmp.path().join("a")),
            loc(&tmp.path().join("a/b")),
            LiveDirection::Both,
        )
        .await
        .is_err());
}

/// LIVE-008 — a side without change notifications is polled.
#[tokio::test]
async fn remote_side_is_polled() {
    use tokio::io::AsyncWriteExt;

    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();

    let mem = MemFs::new("mem");
    mem.seed_dir("/remote");
    let mem: Arc<dyn FileSystem> = Arc::new(mem);
    let registry = Arc::new(Registry::with_local());
    registry.register(mem.clone());

    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(100)), 2);
    let live = LiveSync::new(SyncEngine::new(q.clone(), None), q);
    live.with_poll_interval(Duration::from_millis(300));

    let local_loc = Location::new("local", local.to_string_lossy());
    let mem_loc = Location::new("mem", "/remote");
    live.start(local_loc, mem_loc, LiveDirection::Both)
        .await
        .unwrap();

    // Wait for the first pass to settle.
    eventually("first sync", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= 1)
    })
    .await;

    // A change appears on the remote side; polling must bring it back.
    let mut w = mem.open_write("/remote/new.txt").await.unwrap();
    w.write_all(b"from remote").await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);

    eventually("the remote change is polled", || {
        std::fs::read_to_string(local.join("new.txt"))
            .ok()
            .as_deref()
            == Some("from remote")
    })
    .await;
    live.stop().await;
}

/// LIVE-010 — after stop, later changes are not copied.
#[tokio::test]
async fn stop_ends_the_watcher() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let live = setup();
    live.start(loc(&a), loc(&b), LiveDirection::OneWay)
        .await
        .unwrap();
    eventually("first sync", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= 1)
    })
    .await;
    live.stop().await;

    // A change after stop must not be copied (give the old watcher time to
    // fire if it were still alive).
    std::fs::write(a.join("late.txt"), "late").unwrap();
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert!(!b.join("late.txt").exists(), "a stopped sync must not copy");
}

/// LIVE-009 — a failed sync is broadcast as `Failed` with a message.
#[tokio::test]
async fn failure_is_broadcast() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(local.join("a.txt"), "a").unwrap();

    let remote = Arc::new(FaultyFs::with_id(
        dualpal_core::vfs::LocalFs::new(),
        "remote",
    ));
    let remote_root = tmp.path().join("remote-root");
    std::fs::create_dir_all(&remote_root).unwrap();
    let registry = Arc::new(Registry::with_local());
    registry.register(remote.clone());
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(100)), 2);
    let live = LiveSync::new(SyncEngine::new(q.clone(), None), q);

    live.start(
        Location::new("local", local.to_string_lossy()),
        Location::new("remote", remote_root.to_string_lossy()),
        LiveDirection::OneWay,
    )
    .await
    .unwrap();
    eventually("first sync", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= 1)
    })
    .await;

    // Break every write, then trigger a change.
    remote.break_writes();
    std::fs::write(local.join("b.txt"), "b").unwrap();

    eventually("failed state", || {
        live.status().is_some_and(|s| s.state == LiveState::Failed)
    })
    .await;
    let status = live.status().unwrap();
    assert!(status.error.is_some(), "{:?}", status);
    live.stop().await;
}

/// LIVE-011 — a burst of creations coalesces into a few syncs, all copied.
#[tokio::test]
async fn a_burst_of_creations_is_coalesced() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let queue = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(1000)),
        2,
    );
    let live = LiveSync::new(SyncEngine::new(queue.clone(), None), queue);
    live.with_watch_debounce(Duration::from_millis(800));

    live.start(loc(&a), loc(&b), LiveDirection::OneWay)
        .await
        .unwrap();
    eventually("first sync", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= 1)
    })
    .await;

    for i in 0..500 {
        std::fs::write(a.join(format!("f{i:04}.txt")), "x").unwrap();
    }
    eventually("all 500 copied", || {
        std::fs::read_dir(&b).map(|d| d.count()).unwrap_or(0) >= 500
    })
    .await;

    let syncs = live.status().unwrap().syncs;
    assert!(syncs <= 20, "a burst must coalesce, but ran {syncs} syncs");
    live.stop().await;
}

/// LIVE-013 — each live sync is recorded as an operation in the activity log.
#[tokio::test]
async fn live_syncs_are_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("x.txt"), "x").unwrap();

    let queue = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(100)),
        2,
    );
    let live = LiveSync::new(SyncEngine::new(queue.clone(), None), queue.clone());
    live.start(loc(&a), loc(&b), LiveDirection::OneWay)
        .await
        .unwrap();
    eventually("the copy", || b.join("x.txt").exists()).await;

    eventually("recorded in activity", || {
        queue.activity().list().iter().any(|r| r.kind == "sync")
    })
    .await;
    live.stop().await;
}

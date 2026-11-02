//! Auto-sync of two local folders: changes are picked up as they happen.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dualpal_core::livesync::{LiveDirection, LiveState, LiveSync};
use dualpal_core::sync::SyncEngine;
use dualpal_core::{ActivityLog, Location, OperationQueue, Registry};

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

/// Wait up to 20 s for `cond`.
async fn eventually(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn one_way_copies_changes_as_they_happen() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(a.join("docs")).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("docs/first.txt"), "one").unwrap();
    std::fs::write(b.join("only-here.txt"), "stays").unwrap();

    let live = setup();
    let mut events = live.subscribe();
    let status = live.start(loc(&a), loc(&b), LiveDirection::OneWay).unwrap();
    assert!(status.from_watched && status.to_watched);

    // The first sync copies what is there.
    eventually("the first copy", || b.join("docs/first.txt").exists()).await;
    // A new file is picked up without asking.
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(a.join("docs/second.txt"), "two").unwrap();
    eventually("the new file", || {
        std::fs::read_to_string(b.join("docs/second.txt"))
            .ok()
            .as_deref()
            == Some("two")
    })
    .await;
    // One way never deletes, and leaves the other side's own files alone.
    assert!(b.join("only-here.txt").exists());
    assert!(!a.join("only-here.txt").exists());

    eventually("watching", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.items_synced >= 2)
    })
    .await;
    live.stop();
    assert!(live.status().is_none());
    // The last event says it stopped.
    let mut last = None;
    while let Ok(e) = events.try_recv() {
        last = Some(e);
    }
    assert!(matches!(last, Some(None)));
}

#[tokio::test]
async fn both_ways_copies_each_way_and_passes_on_deletions() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("left.txt"), "L").unwrap();
    std::fs::write(b.join("right.txt"), "R").unwrap();

    let live = setup();
    live.start(loc(&a), loc(&b), LiveDirection::Both).unwrap();
    eventually("both copied", || {
        b.join("left.txt").exists() && a.join("right.txt").exists()
    })
    .await;
    eventually("watching", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching)
    })
    .await;

    // An edit on the right comes back to the left.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(b.join("left.txt"), "edited on the right").unwrap();
    eventually("the edit", || {
        std::fs::read_to_string(a.join("left.txt")).ok().as_deref() == Some("edited on the right")
    })
    .await;
    // A deletion on the left is passed on.
    eventually("watching", || {
        live.status()
            .is_some_and(|s| s.state == LiveState::Watching)
    })
    .await;
    std::fs::remove_file(a.join("right.txt")).unwrap();
    eventually("the deletion", || !b.join("right.txt").exists()).await;
    live.stop();
}

#[tokio::test]
async fn the_same_folder_twice_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let live = setup();
    assert!(live
        .start(loc(tmp.path()), loc(tmp.path()), LiveDirection::Both)
        .is_err());
}

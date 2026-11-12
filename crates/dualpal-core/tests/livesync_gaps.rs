//! Auto-sync cases from §8.9 still missing after Phase 1: renames and a
//! source folder that disappears.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dualpal_core::livesync::{LiveDirection, LiveState, LiveSync};
use dualpal_core::sync::SyncEngine;
use dualpal_core::{ActivityLog, Location, OperationQueue, Registry};
use dualpal_testkit::eventually;

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn setup() -> Arc<LiveSync> {
    let queue = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(100)),
        2,
    );
    let live = LiveSync::new(SyncEngine::new(queue.clone(), None), queue);
    live.with_watch_debounce(Duration::from_millis(100))
        .with_poll_interval(Duration::from_millis(200));
    live
}

const WAIT: Duration = Duration::from_secs(20);

async fn wait_watching(live: &Arc<LiveSync>, syncs: u64) {
    let l = live.clone();
    eventually("auto-sync is watching", WAIT, move || {
        l.status()
            .is_some_and(|s| s.state == LiveState::Watching && s.syncs >= syncs)
    })
    .await;
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// LIVE-012 — renaming a file in the source: a two-way auto-sync adds the
/// new name and removes the old one on the target; one-way only adds.
#[tokio::test]
async fn rename_in_source_is_add_plus_delete_or_add() {
    for (direction, expect) in [
        (LiveDirection::Both, vec!["renamed.txt"]),
        (LiveDirection::OneWay, vec!["original.txt", "renamed.txt"]),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("original.txt"), "content").unwrap();

        let live = setup();
        live.start(loc(&a), loc(&b), direction).await.unwrap();
        wait_watching(&live, 1).await;
        assert_eq!(names(&b), ["original.txt"], "{direction:?}: first sync");

        std::fs::rename(a.join("original.txt"), a.join("renamed.txt")).unwrap();
        let bb = b.clone();
        let want = expect.clone();
        eventually("the rename reaches the target", WAIT, move || {
            names(&bb) == want
        })
        .await;
        assert_eq!(
            std::fs::read_to_string(b.join("renamed.txt")).unwrap(),
            "content"
        );
        live.stop().await;
        assert_eq!(names(&b), expect, "{direction:?}");
    }
}

/// LIVE-014 — when the source folder is deleted while auto-sync runs, it
/// reports a failure and deletes nothing on the target.
#[tokio::test]
async fn deleted_source_folder_fails_without_deleting_the_target() {
    for direction in [LiveDirection::Both, LiveDirection::OneWay] {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("one.txt"), "1").unwrap();
        std::fs::write(a.join("sub/two.txt"), "2").unwrap();

        let live = setup();
        live.start(loc(&a), loc(&b), direction).await.unwrap();
        wait_watching(&live, 1).await;
        assert_eq!(names(&b), ["one.txt", "sub"]);

        std::fs::remove_dir_all(&a).unwrap();
        let l = live.clone();
        eventually("auto-sync reports the failure", WAIT, move || {
            l.status().is_some_and(|s| s.state == LiveState::Failed)
        })
        .await;
        let status = live.status().unwrap();
        assert!(status.error.is_some(), "{direction:?}: {status:?}");
        live.stop().await;

        assert_eq!(names(&b), ["one.txt", "sub"], "{direction:?}");
        assert_eq!(std::fs::read_to_string(b.join("sub/two.txt")).unwrap(), "2");
    }
}

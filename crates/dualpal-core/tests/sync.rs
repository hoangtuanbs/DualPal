//! Folder compare, sync and batch rename, end to end on real folders.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dualpal_core::compare::{compare, CompareBy, CompareOptions, DiffState};
use dualpal_core::rename::RenameItem;
use dualpal_core::sync::{SyncAction, SyncEngine, SyncMode, SyncReason};
use dualpal_core::vfs::S3Fs;
use dualpal_core::{
    ActivityLog, FileSystem, Location, Operation, OperationQueue, OperationStatus, Registry,
};
use tokio_util::sync::CancellationToken;

fn write(root: &Path, rel: &str, content: &str, age_secs: u64) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    let t = SystemTime::now() - Duration::from_secs(age_secs);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn setup() -> (OperationQueue, Arc<Registry>) {
    let registry = Arc::new(Registry::with_local());
    let q = OperationQueue::new(registry.clone(), Arc::new(ActivityLog::in_memory(50)), 2);
    (q, registry)
}

#[tokio::test]
async fn compare_classifies_every_item() {
    let (_, registry) = setup();
    let l = tempfile::tempdir().unwrap();
    let r = tempfile::tempdir().unwrap();
    write(l.path(), "same.txt", "abc", 500);
    write(r.path(), "same.txt", "abc", 500);
    write(l.path(), "newer.txt", "new", 10);
    write(r.path(), "newer.txt", "old", 500);
    write(l.path(), "older.txt", "old", 500);
    write(r.path(), "older.txt", "new!", 10);
    write(l.path(), "sub/only-left.txt", "x", 100);
    write(r.path(), "sub/keep.txt", "k", 100);
    write(l.path(), "sub/keep.txt", "k", 100);
    write(l.path(), "left-dir/a.txt", "a", 100);
    write(r.path(), "clash", "file", 100);
    write(l.path(), "clash/inside.txt", "dir", 100);
    write(l.path(), ".DS_Store", "junk", 100);
    write(l.path(), "skip.tmp", "junk", 100);

    let mut opts = CompareOptions::default();
    opts.exclude.push("*.tmp".into());
    let c = compare(
        &registry,
        &loc(l.path()),
        &loc(r.path()),
        opts,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    let states: HashMap<&str, DiffState> =
        c.items.iter().map(|i| (i.rel.as_str(), i.state)).collect();
    use DiffState::*;
    assert_eq!(states["same.txt"], Same);
    assert_eq!(states["newer.txt"], LeftNewer);
    assert_eq!(states["older.txt"], RightNewer);
    assert_eq!(states["sub"], Different);
    assert_eq!(states["sub/keep.txt"], Same);
    assert_eq!(states["sub/only-left.txt"], LeftOnly);
    assert_eq!(states["left-dir"], LeftOnly);
    assert_eq!(states["left-dir/a.txt"], LeftOnly);
    assert_eq!(states["clash"], KindMismatch);
    assert!(!states.contains_key(".DS_Store"));
    assert!(!states.contains_key("skip.tmp"));
    // Tree order: folders first, contents right after their folder.
    let order: Vec<&str> = c.items.iter().map(|i| i.rel.as_str()).collect();
    assert_eq!(&order[..3], &["clash", "left-dir", "left-dir/a.txt"]);
    assert_eq!(c.summary.left_newer, 1);
    assert_eq!(c.summary.left_only, 2);
    assert_eq!(c.summary.kind_mismatch, 1);

    // By content: same size and date but different bytes.
    write(l.path(), "same.txt", "abd", 500);
    let opts = CompareOptions {
        by: CompareBy::Content,
        ..Default::default()
    };
    let c = compare(
        &registry,
        &loc(l.path()),
        &loc(r.path()),
        opts,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(c.get("same.txt").unwrap().state, Different);
    assert_eq!(c.get("sub/keep.txt").unwrap().state, Same);

    // A folder inside the other is refused.
    let inner = Location::new("local", l.path().join("sub").to_string_lossy());
    assert!(compare(
        &registry,
        &loc(l.path()),
        &inner,
        CompareOptions::default(),
        CancellationToken::new(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn two_way_sync_remembers_what_it_did() {
    let (q, _) = setup();
    let state = tempfile::tempdir().unwrap();
    let engine = SyncEngine::new(q.clone(), Some(state.path().to_path_buf()));
    let l = tempfile::tempdir().unwrap();
    let r = tempfile::tempdir().unwrap();
    let (ll, rl) = (loc(l.path()), loc(r.path()));
    write(l.path(), "a.txt", "from left", 300);
    write(l.path(), "docs/b.txt", "b", 300);
    write(r.path(), "c.txt", "from right", 300);
    write(l.path(), "both.txt", "left version", 10);
    write(r.path(), "both.txt", "older right version", 300);

    // First sync: no history, so nothing is deleted and newer wins.
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
    assert!(!plan.has_history);
    let acts: HashMap<&str, SyncAction> = plan
        .steps
        .iter()
        .map(|s| (s.rel.as_str(), s.action))
        .collect();
    assert_eq!(acts["a.txt"], SyncAction::CopyToRight);
    assert_eq!(acts["docs"], SyncAction::CopyToRight);
    assert_eq!(acts["c.txt"], SyncAction::CopyToLeft);
    assert_eq!(acts["both.txt"], SyncAction::CopyToRight);
    let run = engine.run(plan.id, &HashMap::new()).unwrap();
    // A plan runs once.
    assert!(engine.run(plan.id, &HashMap::new()).is_err());
    run.done.await.unwrap();
    let info = q.get(run.op).unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.kind, "sync");
    assert_eq!(read(r.path(), "docs/b.txt").as_deref(), Some("b"));
    assert_eq!(read(l.path(), "c.txt").as_deref(), Some("from right"));
    assert_eq!(read(r.path(), "both.txt").as_deref(), Some("left version"));

    // In sync now.
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
    assert!(plan.has_history);
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);

    // Delete on the right, edit on the left, edit both sides of one file.
    std::fs::remove_file(r.path().join("a.txt")).unwrap();
    std::fs::remove_dir_all(r.path().join("docs")).unwrap();
    write(l.path(), "c.txt", "edited on the left", 1);
    write(l.path(), "both.txt", "left edit", 1);
    write(r.path(), "both.txt", "right edit!", 1);
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
    let steps: HashMap<&str, _> = plan.steps.iter().map(|s| (s.rel.as_str(), s)).collect();
    assert_eq!(steps["a.txt"].action, SyncAction::DeleteLeft);
    assert_eq!(steps["a.txt"].reason, SyncReason::Deleted);
    assert_eq!(steps["docs"].action, SyncAction::DeleteLeft);
    assert_eq!(steps["c.txt"].action, SyncAction::CopyToRight);
    assert_eq!(steps["both.txt"].action, SyncAction::Skip);
    assert!(steps["both.txt"].conflict);
    // The user resolves the conflict by keeping the right side.
    let choice = HashMap::from([("both.txt".to_string(), SyncAction::CopyToLeft)]);
    let bad = HashMap::from([("a.txt".to_string(), SyncAction::CopyToLeft)]);
    assert!(
        engine.run(plan.id, &bad).is_err(),
        "a.txt is not on the right"
    );
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
    let run = engine.run(plan.id, &choice).unwrap();
    run.done.await.unwrap();
    assert_eq!(q.get(run.op).unwrap().status, OperationStatus::Completed);
    assert!(read(l.path(), "a.txt").is_none());
    assert!(!l.path().join("docs").exists());
    assert_eq!(
        read(r.path(), "c.txt").as_deref(),
        Some("edited on the left")
    );
    assert_eq!(read(l.path(), "both.txt").as_deref(), Some("right edit!"));

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
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);

    // Forgetting the history makes the next sync a first one again.
    engine.forget(&ll, &rl);
    write(r.path(), "new-right.txt", "n", 1);
    std::fs::remove_file(l.path().join("c.txt")).unwrap();
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
    assert!(!plan.has_history);
    assert!(
        plan.steps
            .iter()
            .all(|s| s.action == SyncAction::CopyToLeft),
        "{:?}",
        plan.steps
    );
}

#[tokio::test]
async fn mirror_to_a_bucket_and_replace_a_file_with_a_folder() {
    let (q, registry) = setup();
    let op = opendal::Operator::new(opendal::services::Memory::default())
        .unwrap()
        .finish();
    let bucket: Arc<dyn FileSystem> =
        Arc::new(S3Fs::from_operator("conn:mem".into(), "Memory".into(), op));
    registry.register(bucket.clone());
    bucket.create_dir("/backup").await.unwrap();
    let engine = SyncEngine::new(q.clone(), None);
    let l = tempfile::tempdir().unwrap();
    write(l.path(), "photos/1.jpg", "one", 100);
    write(l.path(), "photos/2.jpg", "two", 100);
    write(l.path(), "notes", "a file for now", 100);
    let ll = loc(l.path());
    let rl = Location::new("conn:mem", "/backup");
    let plan = engine
        .plan(
            &ll,
            &rl,
            SyncMode::Mirror,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let photos = plan.steps.iter().find(|s| s.rel == "photos").unwrap();
    assert_eq!(photos.left.unwrap().files, 2);
    assert_eq!(photos.left.unwrap().bytes, 6);
    let run = engine.run(plan.id, &HashMap::new()).unwrap();
    run.done.await.unwrap();
    let info = q.get(run.op).unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_done, 3);
    assert_eq!(info.progress.bytes_done, 3 + 3 + 14);
    assert_eq!(bucket.stat("/backup/photos/2.jpg").await.unwrap().size, 3);

    // "notes" becomes a folder; an extra file on the bucket goes.
    std::fs::remove_file(l.path().join("notes")).unwrap();
    write(l.path(), "notes/today.md", "hi", 1);
    let mut w = bucket.open_write("/backup/extra.txt").await.unwrap();
    tokio::io::AsyncWriteExt::shutdown(&mut w).await.unwrap();
    drop(w);
    let plan = engine
        .plan(
            &ll,
            &rl,
            SyncMode::Mirror,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let acts: HashMap<&str, SyncAction> = plan
        .steps
        .iter()
        .map(|s| (s.rel.as_str(), s.action))
        .collect();
    assert_eq!(acts["extra.txt"], SyncAction::DeleteRight);
    assert_eq!(
        acts["notes"],
        SyncAction::Skip,
        "a file against a folder needs a decision"
    );
    let choice = HashMap::from([("notes".to_string(), SyncAction::CopyToRight)]);
    let run = engine.run(plan.id, &choice).unwrap();
    run.done.await.unwrap();
    let info = q.get(run.op).unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(bucket.stat("/backup/notes").await.unwrap().is_dir());
    assert_eq!(bucket.stat("/backup/notes/today.md").await.unwrap().size, 2);
    assert!(!bucket.exists("/backup/extra.txt").await.unwrap());

    let plan = engine
        .plan(
            &ll,
            &rl,
            SyncMode::Mirror,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(plan.steps.is_empty(), "{:?}", plan.steps);
}

#[tokio::test]
async fn batch_rename_handles_swaps_and_refuses_clashes() {
    let (q, _) = setup();
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.txt", "A", 1);
    write(d.path(), "b.txt", "B", 1);
    write(d.path(), "c.txt", "C", 1);
    write(d.path(), "taken.txt", "T", 1);
    let at = |n: &str| Location::new("local", d.path().join(n).to_string_lossy());
    let item = |from: &str, to: &str| RenameItem {
        location: at(from),
        new_name: to.into(),
    };
    // Swap a and b, rename c.
    let id = q.enqueue(Operation::BatchRename {
        items: vec![
            item("a.txt", "b.txt"),
            item("b.txt", "a.txt"),
            item("c.txt", "C.txt"),
        ],
    });
    let info = q.wait(id).await.unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(read(d.path(), "a.txt").as_deref(), Some("B"));
    assert_eq!(read(d.path(), "b.txt").as_deref(), Some("A"));
    let names: Vec<String> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"C.txt".to_string()), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with(".dualpal")),
        "{names:?}"
    );

    for items in [
        vec![item("a.txt", "taken.txt")],
        vec![item("a.txt", "x.txt"), item("b.txt", "x.txt")],
        vec![item("a.txt", "bad/name")],
    ] {
        let id = q.enqueue(Operation::BatchRename { items });
        let info = q.wait(id).await.unwrap();
        assert_eq!(info.status, OperationStatus::Failed);
    }
    assert_eq!(read(d.path(), "a.txt").as_deref(), Some("B"));
}

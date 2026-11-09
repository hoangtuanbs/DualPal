//! Remote edit cases that do not need a server (Phase 1, §8.6).

use std::sync::Arc;

use dualpal_core::remote_edit::{EditStatus, RemoteEditor};
use dualpal_core::{FileSystem, Location, Registry};
use dualpal_testkit::MemFs;

/// EDIT-003 — a save moves the session watching → uploading → uploaded, and
/// every change is broadcast.
#[tokio::test]
async fn status_transitions_are_broadcast() {
    let mem = MemFs::new("mem");
    mem.seed_file("/notes.md", "v1");
    let mem: Arc<dyn FileSystem> = Arc::new(mem);
    let registry = Arc::new(Registry::with_local());
    registry.register(mem.clone());
    let tmp = tempfile::tempdir().unwrap();
    let editor = RemoteEditor::new(registry, tmp.path());
    let mut events = editor.subscribe();

    let s = editor
        .start(Location::new("mem", "/notes.md"))
        .await
        .unwrap();
    assert_eq!(s.status, EditStatus::Watching);

    std::fs::write(&s.local_path, "version two").unwrap();

    // Collect the statuses until the upload completes.
    let mut seen = Vec::new();
    let done = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let e = events.recv().await.unwrap();
            seen.push(e.status.clone());
            if e.status == EditStatus::Uploaded {
                return;
            }
        }
    })
    .await;
    done.expect("upload within 10 s");

    // The transitions are ordered.
    let pos = |st: EditStatus| seen.iter().position(|x| *x == st);
    assert!(pos(EditStatus::Watching).is_some());
    assert!(pos(EditStatus::Uploading).is_some());
    assert!(pos(EditStatus::Uploaded).is_some());
    assert!(
        pos(EditStatus::Watching) < pos(EditStatus::Uploading),
        "{seen:?}"
    );
    assert!(
        pos(EditStatus::Uploading) < pos(EditStatus::Uploaded),
        "{seen:?}"
    );

    // The remote now holds the saved content.
    let mut got = String::new();
    use tokio::io::AsyncReadExt;
    mem.open_read("/notes.md")
        .await
        .unwrap()
        .read_to_string(&mut got)
        .await
        .unwrap();
    assert_eq!(got, "version two");
    editor.stop(s.id);
}

/// EDIT-008 — the same file name on two servers is two independent edits.
#[tokio::test]
async fn edits_are_isolated_per_server_and_path() {
    let a = MemFs::new("mem-a");
    a.seed_file("/same.txt", "from A");
    let b = MemFs::new("mem-b");
    b.seed_file("/same.txt", "from B");
    let a: Arc<dyn FileSystem> = Arc::new(a);
    let b: Arc<dyn FileSystem> = Arc::new(b);
    let registry = Arc::new(Registry::with_local());
    registry.register(a.clone());
    registry.register(b.clone());

    let tmp = tempfile::tempdir().unwrap();
    let editor = RemoteEditor::new(registry, tmp.path());
    let sa = editor
        .start(Location::new("mem-a", "/same.txt"))
        .await
        .unwrap();
    let sb = editor
        .start(Location::new("mem-b", "/same.txt"))
        .await
        .unwrap();

    assert_ne!(sa.id, sb.id);
    assert_ne!(
        sa.local_path, sb.local_path,
        "two servers, two local copies"
    );
    assert_eq!(std::fs::read_to_string(&sa.local_path).unwrap(), "from A");
    assert_eq!(std::fs::read_to_string(&sb.local_path).unwrap(), "from B");
    assert_eq!(editor.list().len(), 2);

    // Editing one copy leaves the other untouched.
    std::fs::write(&sa.local_path, "A edited").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_to_string(&sb.local_path).unwrap(), "from B");
    editor.stop(sa.id);
    editor.stop(sb.id);
}

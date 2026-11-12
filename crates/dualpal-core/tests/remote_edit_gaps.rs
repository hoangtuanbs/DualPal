//! Remote edit cases from §8.6 still missing after Phase 1: coalesced
//! saves, stopping, forced uploads and a server copy changed meanwhile.

use std::sync::Arc;
use std::time::Duration;

use dualpal_core::remote_edit::{EditSession, EditStatus, RemoteEditor};
use dualpal_core::{Error, Location, Registry};
use dualpal_testkit::{eventually, FaultOp, FaultyFs, MemFs};

const WAIT: Duration = Duration::from_secs(20);

fn setup(mem: &MemFs) -> (RemoteEditor, tempfile::TempDir) {
    let registry = Arc::new(Registry::with_local());
    registry.register(Arc::new(mem.clone()));
    let tmp = tempfile::tempdir().unwrap();
    (RemoteEditor::new(registry, tmp.path()), tmp)
}

fn session(editor: &RemoteEditor, id: uuid::Uuid) -> Option<EditSession> {
    editor.list().into_iter().find(|s| s.id == id)
}

/// EDIT-004 — several quick saves end with the last content on the server,
/// in at most two uploads.
#[tokio::test]
async fn rapid_saves_coalesce_and_the_last_one_wins() {
    let mem = MemFs::new("mem");
    mem.seed_file("/notes.md", "v0");
    let (editor, _tmp) = setup(&mem);
    let s = editor
        .start(Location::new("mem", "/notes.md"))
        .await
        .unwrap();

    // Five saves within 200 ms, each a different length so each one is a
    // visible change.
    let mut last = String::new();
    for i in 1..=5 {
        last = format!("version {i}{}", "!".repeat(i));
        std::fs::write(&s.local_path, &last).unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await; // spacing between saves
    }

    let m = mem.clone();
    let want = last.clone().into_bytes();
    eventually("the last save is on the server", WAIT, move || {
        m.contents("/notes.md").as_deref() == Some(want.as_slice())
    })
    .await;
    let e = editor.clone();
    eventually("the session is uploaded", WAIT, move || {
        session(&e, s.id).is_some_and(|s| s.status == EditStatus::Uploaded)
    })
    .await;
    let uploads = session(&editor, s.id).unwrap().uploads;
    assert!(
        (1..=2).contains(&uploads),
        "{uploads} uploads for one burst"
    );
    assert_eq!(mem.contents("/notes.md").unwrap(), last.as_bytes());
}

/// EDIT-005 — stopping a session ends it: it is gone from the list, its
/// local copy is removed, and nothing more is uploaded.
#[tokio::test]
async fn stop_session_stops_uploading() {
    let mem = MemFs::new("mem");
    mem.seed_file("/doc.txt", "server");
    let (editor, tmp) = setup(&mem);
    let mut events = editor.subscribe();
    let s = editor
        .start(Location::new("mem", "/doc.txt"))
        .await
        .unwrap();

    assert!(editor.stop(s.id));
    assert!(!editor.stop(s.id), "stopping twice is a no-op");
    assert!(session(&editor, s.id).is_none());
    assert!(
        !std::path::Path::new(&s.local_path).exists(),
        "the local copy is removed"
    );
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        0,
        "no session folder is left"
    );

    let stopped = tokio::time::timeout(WAIT, async {
        loop {
            let e = events.recv().await.unwrap();
            if e.id == s.id && e.status == EditStatus::Stopped {
                return e;
            }
        }
    })
    .await
    .expect("a Stopped event is broadcast");
    assert_eq!(stopped.uploads, 0);

    // Writing a file where the copy was does not reach the server.
    std::fs::create_dir_all(std::path::Path::new(&s.local_path).parent().unwrap()).unwrap();
    std::fs::write(&s.local_path, "after stop").unwrap();
    assert!(
        editor.upload_now(s.id).await.is_err(),
        "the session is gone"
    );
    assert_eq!(mem.contents("/doc.txt").unwrap(), b"server");
}

/// EDIT-006 — `upload_now` sends the local copy at once, also after an
/// upload failed, without waiting for the retry.
#[tokio::test]
async fn upload_now_forces_an_upload() {
    let mem = MemFs::new("inner");
    mem.seed_file("/f.txt", "old");
    let faulty = Arc::new(FaultyFs::with_id(mem.clone(), "mem"));
    let registry = Arc::new(Registry::with_local());
    registry.register(faulty.clone());
    let tmp = tempfile::tempdir().unwrap();
    let editor = RemoteEditor::new(registry, tmp.path());

    let s = editor.start(Location::new("mem", "/f.txt")).await.unwrap();
    faulty.fail_next(
        FaultOp::OpenWrite,
        Error::Connection("server went away".into()),
    );
    std::fs::write(&s.local_path, "new content").unwrap();
    let e = editor.clone();
    eventually("the first upload fails", WAIT, move || {
        session(&e, s.id).is_some_and(|s| s.status == EditStatus::Failed)
    })
    .await;
    assert_eq!(mem.contents("/f.txt").unwrap(), b"old");

    let done = editor.upload_now(s.id).await.unwrap();
    assert_eq!(done.status, EditStatus::Uploaded);
    assert_eq!(done.uploads, 1);
    assert!(done.error.is_none());
    assert_eq!(mem.contents("/f.txt").unwrap(), b"new content");

    // Without a pending change it uploads again anyway.
    let again = editor.upload_now(s.id).await.unwrap();
    assert_eq!(again.uploads, 2);
    editor.stop(s.id);
}

/// EDIT-007 — if the file changed on the server since it was opened, a save
/// must not silently replace the server's version.
#[tokio::test]
#[ignore = "BUG: a save uploads over the server copy without checking whether it changed since the file was opened"]
async fn server_change_is_not_silently_overwritten() {
    let mem = MemFs::new("mem");
    mem.seed_file("/shared.txt", "original");
    let (editor, _tmp) = setup(&mem);
    let s = editor
        .start(Location::new("mem", "/shared.txt"))
        .await
        .unwrap();

    // Someone else changes the server copy, then the local copy is saved.
    mem.seed_file("/shared.txt", "changed by someone else");
    std::fs::write(&s.local_path, "my edit").unwrap();

    let e = editor.clone();
    eventually("the save is handled", WAIT, move || {
        session(&e, s.id)
            .is_some_and(|s| matches!(s.status, EditStatus::Uploaded | EditStatus::Failed))
    })
    .await;
    let after = session(&editor, s.id).unwrap();
    assert_eq!(
        mem.contents("/shared.txt").unwrap(),
        b"changed by someone else",
        "the other change was overwritten; session: {after:?}"
    );
    assert_eq!(after.status, EditStatus::Failed, "the conflict is reported");
}

//! Operation-queue cases from §8.3 that were still missing after Phase 1:
//! kind clashes, symlink loops, parallel limits, unsafe server names,
//! partial deletes, cancelled moves, queue housekeeping and ZIP edge cases.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dualpal_core::vfs::LocalFs;
use dualpal_core::{
    ActivityLog, ConflictPolicy, Location, Operation, OperationInfo, OperationQueue,
    OperationStatus, Registry,
};
use dualpal_testkit::{collect_events, eventually, FaultyFs, MemFs};

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn queue_over(registry: Registry, slots: usize) -> OperationQueue {
    OperationQueue::new(
        Arc::new(registry),
        Arc::new(ActivityLog::in_memory(1000)),
        slots,
    )
}

fn local_queue() -> OperationQueue {
    queue_over(Registry::with_local(), 2)
}

fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

async fn run(q: &OperationQueue, op: Operation) -> OperationInfo {
    let id = q.enqueue(op);
    tokio::time::timeout(Duration::from_secs(60), q.wait(id))
        .await
        .expect("the operation finishes")
        .expect("operation known")
}

fn copy(sources: Vec<Location>, destination: Location, conflict: ConflictPolicy) -> Operation {
    Operation::Copy {
        sources,
        destination,
        conflict,
    }
}

/// Temporary `.dualpal-…` files left in `dir`.
fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("dualpal"))
        .collect()
}

// ---- OPS-COPY-004 -----------------------------------------------------------

/// OPS-COPY-004 — a copied file keeps its permission bits (`script.sh` 0755).
#[cfg(unix)]
#[tokio::test]
#[ignore = "BUG: a copy to a new file does not carry the source's permission bits over; only a replaced file keeps the old target's mode"]
async fn copy_keeps_mode_bits() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("script.sh");
    write(&src, "#!/bin/sh\necho hi\n");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let info = run(
        &local_queue(),
        copy(vec![loc(&src)], loc(&dst), ConflictPolicy::Fail),
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    let mode = std::fs::metadata(dst.join("script.sh"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "copied mode is {mode:o}");
}

// ---- OPS-COPY-010 -----------------------------------------------------------

/// A file `x` to copy, and a destination already holding a folder `x`
/// with one file in it.
fn file_over_folder(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let src = root.join("src/x");
    write(&src, "new file");
    let dst = root.join("dst");
    write(&dst.join("x/inner.txt"), "keep me");
    (src, dst)
}

/// OPS-COPY-010 — a file over a folder of the same name: `fail` refuses with
/// `alreadyExists`, `skip` leaves the folder, `overwrite` either replaces it
/// cleanly or fails clearly. The folder is never lost silently.
#[tokio::test]
async fn file_over_folder_never_loses_the_folder_silently() {
    // Fail
    let tmp = tempfile::tempdir().unwrap();
    let (src, dst) = file_over_folder(tmp.path());
    let info = run(
        &local_queue(),
        copy(vec![loc(&src)], loc(&dst), ConflictPolicy::Fail),
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "alreadyExists");
    assert_eq!(
        std::fs::read_to_string(dst.join("x/inner.txt")).unwrap(),
        "keep me"
    );

    // Skip
    let tmp = tempfile::tempdir().unwrap();
    let (src, dst) = file_over_folder(tmp.path());
    let info = run(
        &local_queue(),
        copy(vec![loc(&src)], loc(&dst), ConflictPolicy::Skip),
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.skipped, 1);
    assert_eq!(
        std::fs::read_to_string(dst.join("x/inner.txt")).unwrap(),
        "keep me"
    );

    // Overwrite
    let tmp = tempfile::tempdir().unwrap();
    let (src, dst) = file_over_folder(tmp.path());
    let info = run(
        &local_queue(),
        copy(vec![loc(&src)], loc(&dst), ConflictPolicy::Overwrite),
    )
    .await;
    let x = dst.join("x");
    match info.status {
        OperationStatus::Completed => {
            assert!(x.is_file(), "a completed overwrite leaves the new file");
            assert_eq!(std::fs::read_to_string(&x).unwrap(), "new file");
        }
        OperationStatus::Failed => {
            assert!(info.error.is_some(), "a failure says why");
            assert_eq!(
                std::fs::read_to_string(x.join("inner.txt")).unwrap(),
                "keep me",
                "a failed overwrite keeps the folder"
            );
        }
        other => panic!("unexpected status {other:?}"),
    }
    assert!(leftovers(&dst).is_empty(), "{:?}", leftovers(&dst));
}

/// OPS-COPY-010 — a folder over a file of the same name: every policy keeps
/// the file, and `fail` reports `alreadyExists`.
#[tokio::test]
async fn folder_over_file_never_loses_the_file_silently() {
    for conflict in [
        ConflictPolicy::Fail,
        ConflictPolicy::Skip,
        ConflictPolicy::Overwrite,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/x");
        write(&src.join("child.txt"), "child");
        let dst = tmp.path().join("dst");
        write(&dst.join("x"), "existing file");

        let info = run(&local_queue(), copy(vec![loc(&src)], loc(&dst), conflict)).await;
        let x = dst.join("x");
        if conflict == ConflictPolicy::Fail {
            assert_eq!(info.status, OperationStatus::Failed, "{conflict:?}");
            assert_eq!(info.error.as_ref().unwrap().kind, "alreadyExists");
        }
        if info.status == OperationStatus::Failed {
            assert!(info.error.is_some(), "{conflict:?}: a failure says why");
        }
        if x.is_file() {
            assert_eq!(
                std::fs::read_to_string(&x).unwrap(),
                "existing file",
                "{conflict:?}"
            );
        } else {
            // Only a completed overwrite may replace the file with the folder.
            assert_eq!(conflict, ConflictPolicy::Overwrite);
            assert_eq!(info.status, OperationStatus::Completed);
            assert_eq!(
                std::fs::read_to_string(x.join("child.txt")).unwrap(),
                "child"
            );
        }
    }
}

// ---- OPS-COPY-011 -----------------------------------------------------------

/// OPS-COPY-011 — links to folders, including one to its own parent, are not
/// followed: the copy terminates and nothing is copied through them.
/// (Links to folders are skipped, not recreated as links.)
#[tokio::test]
async fn copy_does_not_follow_folder_links_into_a_loop() {
    let src = MemFs::new("src");
    src.seed_file("/docs/readme.txt", "docs");
    src.seed_file("/tree/real.txt", "real");
    src.seed_symlink("/tree/link-to-docs", "/docs");
    src.seed_symlink("/tree/loop", "/tree");
    let dst = MemFs::new("dst");
    dst.seed_dir("/out");
    let registry = Registry::new();
    registry.register(Arc::new(src));
    registry.register(Arc::new(dst.clone()));
    let q = queue_over(registry, 2);

    let info = run(
        &q,
        copy(
            vec![Location::new("src", "/tree")],
            Location::new("dst", "/out"),
            ConflictPolicy::Fail,
        ),
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(dst.contents("/out/tree/real.txt").unwrap(), b"real");
    assert!(info.progress.skipped >= 2, "{:?}", info.progress);
    let copied: Vec<String> = dst
        .paths()
        .into_iter()
        .filter(|p| p.starts_with("/out/"))
        .collect();
    assert_eq!(copied, ["/out/tree", "/out/tree/real.txt"], "{copied:?}");
}

// ---- OPS-COPY-015 -----------------------------------------------------------

/// Copy eight files to a "remote" disk that allows `limit` transfers at once
/// and whose writers hold off their first byte, so every worker that starts
/// is open at the same moment. Returns the most writers seen at once.
async fn most_writers_with_limit(limit: usize) -> usize {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for i in 0..8 {
        write(&src.join(format!("f{i}.txt")), &format!("file {i}"));
    }
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_parallel_transfers(limit);
    remote.slow_writes(Duration::from_millis(250));
    let registry = Registry::with_local();
    registry.register(remote.clone());
    let q = queue_over(registry, 2);

    let info = run(
        &q,
        copy(
            vec![loc(&src)],
            Location::new("remote", dst.to_string_lossy()),
            ConflictPolicy::Fail,
        ),
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_done, 8);
    remote.most_parallel_writers()
}

/// OPS-COPY-015 — the queue copies exactly as many files at once as the
/// backend's `parallel_transfers()` allows: 1, 2 (FTP-like) or 4 (SFTP-like).
#[tokio::test]
async fn parallel_transfers_are_respected() {
    for limit in [1, 2, 4] {
        assert_eq!(
            most_writers_with_limit(limit).await,
            limit,
            "parallel_transfers() = {limit}"
        );
    }
}

// ---- OPS-COPY-024 -----------------------------------------------------------

/// OPS-COPY-024, SEC-001 — server names that are not plain local file names never
/// write outside the destination: the copy is refused or the name is made
/// safe, and the folder around the destination is untouched.
#[tokio::test]
async fn unsafe_server_names_stay_inside_the_destination() {
    let mut names = vec!["a\\b", "x:y", "ok.txt"];
    if cfg!(windows) {
        names.push("con");
    }
    for name in names {
        let server = MemFs::new("server");
        server.seed_file(&format!("/in/{name}"), "payload");
        let registry = Registry::with_local();
        registry.register(Arc::new(server));
        let q = queue_over(registry, 2);

        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join("dst");
        std::fs::create_dir(&dst).unwrap();
        let info = run(
            &q,
            copy(
                vec![Location::new("server", "/in")],
                loc(&dst),
                ConflictPolicy::Fail,
            ),
        )
        .await;

        let outside: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "dst")
            .collect();
        assert!(outside.is_empty(), "{name:?} wrote outside: {outside:?}");
        match info.status {
            OperationStatus::Completed => {
                let inside: Vec<_> = std::fs::read_dir(dst.join("in"))
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                assert_eq!(inside.len(), 1, "{name:?}: {inside:?}");
                assert_eq!(std::fs::read_to_string(&inside[0]).unwrap(), "payload");
            }
            OperationStatus::Failed => {
                assert_ne!(name, "ok.txt");
                assert_eq!(info.error.unwrap().kind, "invalid", "{name:?}");
            }
            other => panic!("{name:?}: unexpected status {other:?}"),
        }
    }
}

// ---- OPS-DEL-003 ------------------------------------------------------------

/// Keeps one item from being deleted until dropped: on Unix its folder is
/// read-only, on Windows the file is held open without delete sharing
/// (Windows deletes read-only files these days).
struct Blocker {
    item: std::path::PathBuf,
    #[cfg(windows)]
    _handle: std::fs::File,
}

impl Blocker {
    fn new(item: std::path::PathBuf) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = item.parent().unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            Self { item }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 1;
            let handle = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&item)
                .unwrap();
            Self {
                item,
                _handle: handle,
            }
        }
    }

    /// Whether the OS actually refuses to delete the item (root ignores
    /// Unix permission bits, and then there is nothing to test).
    fn enforced(&self) -> bool {
        let probe = self.item.with_file_name("probe");
        let refused = std::fs::write(&probe, "x").is_err();
        let _ = std::fs::remove_file(&probe);
        cfg!(windows) || refused
    }
}

impl Drop for Blocker {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = self.item.parent().unwrap();
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
    }
}

/// Three targets for a delete; the middle one holds an item that cannot be
/// removed while the returned [`Blocker`] lives.
fn partial_delete_tree(root: &Path) -> (Vec<std::path::PathBuf>, Blocker) {
    let first = root.join("first.txt");
    write(&first, "1");
    let blocked_dir = root.join("blocked");
    let stuck = blocked_dir.join("sub/stuck.txt");
    write(&stuck, "stuck");
    let last = root.join("last.txt");
    write(&last, "3");
    (vec![first, blocked_dir, last], Blocker::new(stuck))
}

/// OPS-DEL-003 — a delete that hits an item it cannot remove fails, and the
/// error names that item.
#[tokio::test]
async fn partial_delete_failure_names_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (targets, blocker) = partial_delete_tree(tmp.path());
    if !blocker.enforced() {
        eprintln!("skipped: permission bits are not enforced here");
        return;
    }
    let info = run(
        &local_queue(),
        Operation::Delete {
            targets: targets.iter().map(|p| loc(p)).collect(),
        },
    )
    .await;
    assert!(blocker.item.exists(), "the blocked item is still there");
    drop(blocker);

    assert_eq!(info.status, OperationStatus::Failed);
    let err = info.error.unwrap();
    if cfg!(unix) {
        assert_eq!(err.kind, "permissionDenied", "{err:?}");
    }
    assert!(
        err.message.contains("stuck.txt") || err.message.contains("sub"),
        "the error names the item: {err:?}"
    );
}

/// OPS-DEL-003 — the other targets are deleted even when one of them fails.
#[tokio::test]
#[ignore = "BUG: a delete stops at the first item it cannot remove, so the targets after it in the queue's order are left in place"]
async fn partial_delete_failure_still_deletes_the_other_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let (targets, blocker) = partial_delete_tree(tmp.path());
    if !blocker.enforced() {
        eprintln!("skipped: permission bits are not enforced here");
        return;
    }
    let info = run(
        &local_queue(),
        Operation::Delete {
            targets: targets.iter().map(|p| loc(p)).collect(),
        },
    )
    .await;
    drop(blocker);

    assert_eq!(info.status, OperationStatus::Failed);
    assert!(!targets[0].exists(), "first target deleted");
    assert!(!targets[2].exists(), "last target deleted");
}

// ---- OPS-MOVE-004 -----------------------------------------------------------

/// OPS-MOVE-004 — cancelling a move to another file system while files are
/// being written keeps every source file, and leaves no partial files.
#[tokio::test]
async fn cancelled_cross_fs_move_keeps_the_source() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for i in 0..6 {
        write(&src.join(format!("f{i}.txt")), &format!("content {i}"));
    }
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_parallel_transfers(2);
    remote.slow_writes(Duration::from_secs(3));
    let registry = Registry::with_local();
    registry.register(remote.clone());
    let q = queue_over(registry, 2);

    let id = q.enqueue(Operation::Move {
        sources: vec![loc(&src)],
        destination: Location::new("remote", dst.to_string_lossy()),
        conflict: ConflictPolicy::Fail,
    });
    let r = remote.clone();
    eventually(
        "a file is being written",
        Duration::from_secs(10),
        move || r.open_writers() > 0,
    )
    .await;
    assert!(q.cancel(id));
    let info = tokio::time::timeout(Duration::from_secs(10), q.wait(id))
        .await
        .expect("cancel is prompt")
        .unwrap();

    assert_eq!(info.status, OperationStatus::Cancelled);
    for i in 0..6 {
        assert_eq!(
            std::fs::read_to_string(src.join(format!("f{i}.txt"))).unwrap(),
            format!("content {i}"),
            "source f{i} is kept"
        );
    }
    let moved = dst.join("src");
    if moved.exists() {
        assert!(leftovers(&moved).is_empty(), "{:?}", leftovers(&moved));
    }
}

// ---- OPS-Q-004 / OPS-Q-008 --------------------------------------------------

/// OPS-Q-004 — clearing finished operations keeps the running and queued ones.
#[tokio::test]
async fn clear_finished_keeps_running_and_queued() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.slow_writes(Duration::from_secs(3));
    let registry = Registry::with_local();
    registry.register(remote.clone());
    let q = queue_over(registry, 1);

    let done = run(
        &q,
        Operation::CreateDir {
            location: loc(&tmp.path().join("done")),
        },
    )
    .await;
    assert_eq!(done.status, OperationStatus::Completed);

    let src = tmp.path().join("slow.txt");
    write(&src, "slow");
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    let running = q.enqueue(copy(
        vec![loc(&src)],
        Location::new("remote", dst.to_string_lossy()),
        ConflictPolicy::Fail,
    ));
    let queued = q.enqueue(Operation::CreateDir {
        location: loc(&tmp.path().join("later")),
    });
    let r = remote.clone();
    eventually("the copy is running", Duration::from_secs(10), move || {
        r.open_writers() > 0
    })
    .await;

    q.clear_finished();
    let ids: Vec<_> = q.list().into_iter().map(|i| i.id).collect();
    assert!(!ids.contains(&done.id), "finished operation is cleared");
    assert!(ids.contains(&running), "running operation is kept");
    assert!(ids.contains(&queued), "queued operation is kept");
    assert_eq!(q.get(running).unwrap().status, OperationStatus::Running);
    assert_eq!(q.get(queued).unwrap().status, OperationStatus::Queued);
    // The cleared one is still in the activity log.
    assert!(q.activity().list().iter().any(|r| r.id == done.id));

    q.cancel(queued);
    q.cancel(running);
    q.wait(running).await;
    q.wait(queued).await;
}

/// OPS-Q-008 — fifty operations enqueued at once all finish, never more than
/// two running at a time.
#[tokio::test]
async fn fifty_operations_all_finish_two_at_a_time() {
    let tmp = tempfile::tempdir().unwrap();
    let q = local_queue();
    let rec = collect_events(&q);
    let ids: Vec<_> = (0..50)
        .map(|i| {
            q.enqueue(Operation::CreateDir {
                location: loc(&tmp.path().join(format!("d{i:02}"))),
            })
        })
        .collect();
    for id in &ids {
        let info = tokio::time::timeout(Duration::from_secs(30), q.wait(*id))
            .await
            .expect("no deadlock")
            .unwrap();
        assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    }
    eventually("recorder catches up", Duration::from_secs(5), || {
        rec.events()
            .iter()
            .filter(|e| e.status.is_finished())
            .count()
            >= 50
    })
    .await;

    let mut running = std::collections::HashSet::new();
    let mut most = 0;
    for e in rec.events() {
        match e.status {
            OperationStatus::Running => {
                running.insert(e.id);
            }
            s if s.is_finished() => {
                running.remove(&e.id);
            }
            _ => {}
        }
        most = most.max(running.len());
    }
    assert!(most <= 2, "{most} operations ran at once");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 50);
}

// ---- OPS-ZIP-002 / OPS-ZIP-004 ----------------------------------------------

/// OPS-ZIP-002 — Unicode names, empty folders and empty files survive a
/// round trip through a ZIP archive.
#[allow(deprecated)] // async_zip's tokio file reader, fine for a test
#[tokio::test]
async fn compress_keeps_unicode_names_empty_folders_and_empty_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("Ünïcødé");
    write(&src.join("日本語.txt"), "こんにちは");
    write(&src.join("empty.bin"), "");
    std::fs::create_dir_all(src.join("vide/leer")).unwrap();
    write(&src.join("emoji 🎉.md"), "party");
    let zip_path = tmp.path().join("out.zip");

    let info = run(
        &local_queue(),
        Operation::Compress {
            sources: vec![loc(&src)],
            destination: loc(&zip_path),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);

    let reader = async_zip::tokio::read::fs::ZipFileReader::new(&zip_path)
        .await
        .unwrap();
    let entries = reader.file().entries();
    let mut names: Vec<String> = entries
        .iter()
        .map(|e| e.filename().as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Ünïcødé/",
            "Ünïcødé/emoji 🎉.md",
            "Ünïcødé/empty.bin",
            "Ünïcødé/vide/",
            "Ünïcødé/vide/leer/",
            "Ünïcødé/日本語.txt",
        ]
    );
    for (name, want) in [
        ("Ünïcødé/日本語.txt", "こんにちは"),
        ("Ünïcødé/empty.bin", ""),
        ("Ünïcødé/emoji 🎉.md", "party"),
    ] {
        let idx = entries
            .iter()
            .position(|e| e.filename().as_str().unwrap() == name)
            .unwrap();
        let mut entry = reader.reader_with_entry(idx).await.unwrap();
        let mut out = String::new();
        entry.read_to_string_checked(&mut out).await.unwrap();
        assert_eq!(out, want, "{name}");
    }
}

/// OPS-ZIP-004 — compressing onto an existing file is `alreadyExists` and the
/// file is left as it was.
#[tokio::test]
async fn compress_onto_an_existing_file_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("a.txt");
    write(&src, "a");
    let existing = tmp.path().join("taken.zip");
    write(&existing, "not a zip, but mine");

    let info = run(
        &local_queue(),
        Operation::Compress {
            sources: vec![loc(&src)],
            destination: loc(&existing),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "alreadyExists");
    assert_eq!(
        std::fs::read_to_string(&existing).unwrap(),
        "not a zip, but mine"
    );
}

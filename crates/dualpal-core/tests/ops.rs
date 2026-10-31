use std::path::Path;
use std::sync::Arc;

use dualpal_core::{
    ActivityLog, ConflictPolicy, Location, Operation, OperationQueue, OperationStatus, Registry,
};

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

fn queue() -> OperationQueue {
    OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(100)),
        2,
    )
}

fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// src/
///   a.txt          "alpha"
///   sub/b.txt      "bravo!"
///   sub/deep/c.txt "charlie"
fn sample_tree(root: &Path) -> std::path::PathBuf {
    let src = root.join("src");
    write(&src.join("a.txt"), "alpha");
    write(&src.join("sub/b.txt"), "bravo!");
    write(&src.join("sub/deep/c.txt"), "charlie");
    src
}

async fn run(q: &OperationQueue, op: Operation) -> dualpal_core::OperationInfo {
    let id = q.enqueue(op);
    q.wait(id).await.expect("operation known")
}

#[tokio::test]
async fn copies_a_folder_tree_with_progress_totals() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let q = queue();
    let info = run(
        &q,
        Operation::Copy {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_total, 3);
    assert_eq!(info.progress.files_done, 3);
    assert_eq!(info.progress.bytes_total, 5 + 6 + 7);
    assert_eq!(info.progress.bytes_done, 18);
    assert_eq!(
        std::fs::read_to_string(dst.join("src/sub/deep/c.txt")).unwrap(),
        "charlie"
    );
    assert!(src.join("a.txt").exists(), "copy keeps the source");

    let log = q.activity().list();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, "copy");
    assert_eq!(log[0].status, OperationStatus::Completed);
}

#[tokio::test]
async fn copy_preserves_modification_time() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("old.txt");
    write(&file, "x");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&file)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed);
    assert_eq!(
        std::fs::metadata(dst.join("old.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        old
    );
}

#[tokio::test]
async fn conflict_policies() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("f.txt");
    write(&file, "new");
    let dst = tmp.path().join("dst");
    write(&dst.join("f.txt"), "old");
    let q = queue();
    let copy = |conflict| Operation::Copy {
        sources: vec![loc(&file)],
        destination: loc(&dst),
        conflict,
    };

    let failed = run(&q, copy(ConflictPolicy::Fail)).await;
    assert_eq!(failed.status, OperationStatus::Failed);
    assert_eq!(failed.error.unwrap().kind, "alreadyExists");
    assert_eq!(std::fs::read_to_string(dst.join("f.txt")).unwrap(), "old");

    let skipped = run(&q, copy(ConflictPolicy::Skip)).await;
    assert_eq!(skipped.status, OperationStatus::Completed);
    assert_eq!(skipped.progress.skipped, 1);
    assert_eq!(std::fs::read_to_string(dst.join("f.txt")).unwrap(), "old");

    let replaced = run(&q, copy(ConflictPolicy::Overwrite)).await;
    assert_eq!(replaced.status, OperationStatus::Completed);
    assert_eq!(std::fs::read_to_string(dst.join("f.txt")).unwrap(), "new");
}

#[tokio::test]
async fn refuses_to_copy_a_folder_into_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&src)],
            destination: loc(&src.join("sub")),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "invalid");
}

#[tokio::test]
async fn refuses_to_copy_onto_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("f.txt");
    write(&file, "keep me");
    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&file)],
            destination: loc(tmp.path()),
            conflict: ConflictPolicy::Overwrite,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
}

#[tokio::test]
async fn moves_within_the_same_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let info = run(
        &queue(),
        Operation::Move {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(!src.exists());
    assert_eq!(
        std::fs::read_to_string(dst.join("src/sub/b.txt")).unwrap(),
        "bravo!"
    );
}

#[tokio::test]
async fn move_merge_keeps_skipped_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let dst = tmp.path().join("dst");
    write(&dst.join("src/a.txt"), "existing");

    // Target folder exists, so the rename fast path is not taken: overwrite merges.
    let q = queue();
    let info = run(
        &q,
        Operation::Move {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Overwrite,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(!src.exists());
    assert_eq!(
        std::fs::read_to_string(dst.join("src/a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        std::fs::read_to_string(dst.join("src/sub/deep/c.txt")).unwrap(),
        "charlie"
    );

    // With Skip the whole item is left alone.
    let src2 = sample_tree(&tmp.path().join("second"));
    let dst2 = tmp.path().join("dst2");
    write(&dst2.join("src/a.txt"), "existing");
    let info = run(
        &q,
        Operation::Move {
            sources: vec![loc(&src2)],
            destination: loc(&dst2),
            conflict: ConflictPolicy::Skip,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed);
    assert_eq!(info.progress.skipped, 1);
    assert!(src2.join("a.txt").exists());
}

#[tokio::test]
async fn deletes_recursively() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let lone = tmp.path().join("lone.txt");
    write(&lone, "x");

    let info = run(
        &queue(),
        Operation::Delete {
            targets: vec![loc(&src), loc(&lone)],
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    // 3 files + 3 folders + lone.txt
    assert_eq!(info.progress.files_total, 7);
    assert_eq!(info.progress.files_done, 7);
    assert!(!src.exists());
    assert!(!lone.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn delete_does_not_follow_symlinked_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let keep = sample_tree(&tmp.path().join("keep"));
    let victim = tmp.path().join("victim");
    std::fs::create_dir(&victim).unwrap();
    std::os::unix::fs::symlink(&keep, victim.join("link")).unwrap();

    let info = run(
        &queue(),
        Operation::Delete {
            targets: vec![loc(&victim)],
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(!victim.exists());
    assert!(
        keep.join("sub/deep/c.txt").exists(),
        "link target must survive"
    );
}

#[tokio::test]
async fn creates_and_renames() {
    let tmp = tempfile::tempdir().unwrap();
    let q = queue();
    let dir = tmp.path().join("New Folder");
    let info = run(
        &q,
        Operation::CreateDir {
            location: loc(&dir),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed);
    assert!(dir.is_dir());

    let info = run(
        &q,
        Operation::Rename {
            location: loc(&dir),
            new_name: "Renamed".into(),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(tmp.path().join("Renamed").is_dir());

    let bad = run(
        &q,
        Operation::Rename {
            location: loc(&tmp.path().join("Renamed")),
            new_name: "a/b".into(),
        },
    )
    .await;
    assert_eq!(bad.status, OperationStatus::Failed);
}

#[tokio::test]
async fn cancels_a_running_copy_and_cleans_up() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("big.bin");
    std::fs::write(&big, vec![7u8; 64 * 1024 * 1024]).unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let q = queue();
    let mut rx = q.subscribe();
    let id = q.enqueue(Operation::Copy {
        sources: vec![loc(&big)],
        destination: loc(&dst),
        conflict: ConflictPolicy::Fail,
    });
    // Cancel as soon as it is running.
    while let Ok(info) = rx.recv().await {
        if info.id == id && info.status == OperationStatus::Running {
            assert!(q.cancel(id));
            break;
        }
    }
    let info = q.wait(id).await.unwrap();
    assert_eq!(info.status, OperationStatus::Cancelled);
    assert!(!dst.join("big.bin").exists(), "partial file removed");
    assert!(!q.cancel(id), "cannot cancel a finished operation");
}

#[tokio::test]
async fn operations_wait_for_a_free_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let q = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(10)),
        1,
    );
    let ids: Vec<_> = (0..5)
        .map(|i| {
            q.enqueue(Operation::CreateDir {
                location: loc(&tmp.path().join(format!("d{i}"))),
            })
        })
        .collect();
    for id in ids {
        assert_eq!(q.wait(id).await.unwrap().status, OperationStatus::Completed);
    }
    assert_eq!(q.activity().list().len(), 5);
}

#[test]
fn operation_json_shape() {
    let op: Operation = serde_json::from_str(
        r#"{"type":"copy","sources":[{"fs":"local","path":"/a"}],"destination":{"fs":"local","path":"/b"}}"#,
    )
    .unwrap();
    assert_eq!(
        op,
        Operation::Copy {
            sources: vec![Location::new("local", "/a")],
            destination: Location::new("local", "/b"),
            conflict: ConflictPolicy::Fail
        }
    );
    let rename: Operation = serde_json::from_str(
        r#"{"type":"rename","location":{"fs":"local","path":"/a"},"newName":"b"}"#,
    )
    .unwrap();
    assert_eq!(rename.kind(), "rename");
}

#[tokio::test]
async fn moves_items_to_trash() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("trash-me.txt");
    write(&file, "bye");
    let info = run(
        &queue(),
        Operation::Trash {
            targets: vec![loc(&file)],
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(!file.exists());

    let missing = run(
        &queue(),
        Operation::Trash {
            targets: vec![loc(&tmp.path().join("nope"))],
        },
    )
    .await;
    assert_eq!(missing.status, OperationStatus::Failed);
    assert_eq!(missing.error.unwrap().kind, "notFound");
}

// async_zip's only tokio file reader is marked deprecated in favour of an
// unreleased API; it is fine for verifying archives in a test.
#[allow(deprecated)]
#[tokio::test]
async fn compresses_files_and_folders_into_a_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let src = sample_tree(tmp.path());
    let lone = tmp.path().join("lone.txt");
    write(&lone, "solo");
    let zip_path = tmp.path().join("Archive.zip");

    let q = queue();
    let op = Operation::Compress {
        sources: vec![loc(&src), loc(&lone)],
        destination: loc(&zip_path),
    };
    let info = run(&q, op.clone()).await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_done, 4);
    assert_eq!(info.progress.bytes_done, 18 + 4);

    let reader = async_zip::tokio::read::fs::ZipFileReader::new(&zip_path)
        .await
        .unwrap();
    let mut names: Vec<String> = reader
        .file()
        .entries()
        .iter()
        .map(|e| e.filename().as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "lone.txt",
            "src/",
            "src/a.txt",
            "src/sub/",
            "src/sub/b.txt",
            "src/sub/deep/",
            "src/sub/deep/c.txt"
        ]
    );
    let idx = reader
        .file()
        .entries()
        .iter()
        .position(|e| e.filename().as_str().unwrap() == "src/sub/deep/c.txt")
        .unwrap();
    let mut entry = reader.reader_with_entry(idx).await.unwrap();
    let mut out = String::new();
    entry.read_to_string_checked(&mut out).await.unwrap();
    assert_eq!(out, "charlie");

    // Never overwrites an existing archive.
    let again = run(&q, op).await;
    assert_eq!(again.error.unwrap().kind, "alreadyExists");
}

#[tokio::test]
async fn reads_and_saves_text() {
    use dualpal_core::vfs::{read_text, write_text, LocalFs};
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("notes.md");
    write(&file, "héllo world");
    let fs = LocalFs::new();
    let p = file.to_string_lossy();

    let t = read_text(&fs, &p, 1000).await.unwrap();
    assert_eq!(
        (t.text.as_str(), t.truncated, t.binary),
        ("héllo world", false, false)
    );
    // Limit falls inside the two-byte "é": still valid text, marked truncated.
    let t = read_text(&fs, &p, 2).await.unwrap();
    assert_eq!((t.text.as_str(), t.truncated, t.binary), ("h", true, false));

    write_text(&fs, &p, "saved ✓").await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "saved ✓");
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        1,
        "no temp file left behind"
    );

    let bin = tmp.path().join("x.bin");
    std::fs::write(&bin, [0u8, 159, 146, 150]).unwrap();
    assert!(
        read_text(&fs, &bin.to_string_lossy(), 100)
            .await
            .unwrap()
            .binary
    );
}

#[cfg(unix)]
#[tokio::test]
async fn changes_permissions_and_saving_keeps_them() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("script.sh");
    write(&file, "echo hi");
    let info = run(
        &queue(),
        Operation::SetMode {
            location: loc(&file),
            mode: 0o750,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o750
    );

    dualpal_core::vfs::write_text(
        &dualpal_core::vfs::LocalFs::new(),
        &file.to_string_lossy(),
        "echo bye",
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

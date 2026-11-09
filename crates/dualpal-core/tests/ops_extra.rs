//! New operation-queue cases (Phase 1, §8.3) that did not exist before.

use std::path::Path;
use std::sync::Arc;

use dualpal_core::{
    ActivityLog, ConflictPolicy, Location, Operation, OperationQueue, OperationStatus, Registry,
};
use dualpal_testkit::{collect_events, eventually, MemFs};

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

async fn run(q: &OperationQueue, op: Operation) -> dualpal_core::OperationInfo {
    let id = q.enqueue(op);
    q.wait(id).await.expect("operation known")
}

/// OPS-COPY-002 — the progress stream is monotonic and reaches its totals.
#[tokio::test]
async fn copy_progress_is_monotonic() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src.join("a.txt"), "aaaa");
    write(&src.join("b.txt"), "bbbbbb");
    write(&src.join("sub/c.txt"), "cccc");
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let q = queue();
    let rec = collect_events(&q);
    let info = run(
        &q,
        Operation::Copy {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;

    // Wait until the recorder has seen the terminal event too.
    eventually(
        "recorder catches up",
        std::time::Duration::from_secs(5),
        || {
            rec.for_op(info.id)
                .last()
                .is_some_and(|e| e.status.is_finished())
        },
    )
    .await;

    let stream = rec.for_op(info.id);
    let statuses: Vec<_> = stream.iter().map(|e| e.status).collect();
    assert_eq!(statuses.first(), Some(&OperationStatus::Queued));
    assert_eq!(statuses.last(), Some(&OperationStatus::Completed));

    let mut prev_bytes = 0u64;
    let mut prev_files = 0u64;
    for e in &stream {
        assert!(
            e.progress.bytes_done >= prev_bytes,
            "bytes_done went backwards: {e:?}"
        );
        assert!(e.progress.files_done >= prev_files);
        prev_bytes = e.progress.bytes_done;
        prev_files = e.progress.files_done;
    }
    let last = stream.last().unwrap();
    assert_eq!(last.progress.bytes_done, last.progress.bytes_total);
    assert_eq!(last.progress.files_done, last.progress.files_total);
}

/// OPS-COPY-006 — with `Fail`, a conflict stops before the remaining sources.
#[tokio::test]
async fn conflict_fail_stops_before_any_further_write() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.txt");
    let b = tmp.path().join("b.txt");
    let c = tmp.path().join("c.txt");
    write(&a, "new a");
    write(&b, "new b");
    write(&c, "new c");
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    write(&dst.join("b.txt"), "old b");

    let q = queue();
    let info = run(
        &q,
        Operation::Copy {
            sources: vec![loc(&a), loc(&b), loc(&c)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;

    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "alreadyExists");
    // The conflicting file is untouched; the last source was never copied.
    assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "new a");
    assert_eq!(std::fs::read_to_string(dst.join("b.txt")).unwrap(), "old b");
    assert!(
        !dst.join("c.txt").exists(),
        "the third source must not be copied"
    );
}

/// OPS-COPY-009 — copying a folder over an existing folder merges.
#[tokio::test]
async fn folder_merge_keeps_the_existing_extra_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src.join("a.txt"), "a");
    let dst = tmp.path().join("dst");
    write(&dst.join("src/extra.txt"), "extra");

    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(std::fs::read_to_string(dst.join("src/a.txt")).unwrap(), "a");
    assert_eq!(
        std::fs::read_to_string(dst.join("src/extra.txt")).unwrap(),
        "extra"
    );
}

/// OPS-COPY-012 — an empty file and an empty folder are copied.
#[tokio::test]
async fn empty_file_and_folder_are_copied() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src.join("empty.txt"), "");
    std::fs::create_dir_all(src.join("empty-dir")).unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();

    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&src)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(dst.join("src/empty.txt").exists());
    assert!(dst.join("src/empty-dir").is_dir());
}

/// OPS-MOVE-002 — moving across file systems copies then deletes the source.
#[tokio::test]
async fn cross_fs_move_copies_then_deletes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src.join("a.txt"), "alpha");

    let mem = MemFs::with_default_id();
    mem.seed_dir("/dst");
    let mem: Arc<dyn dualpal_core::FileSystem> = Arc::new(mem);
    let registry = Arc::new(Registry::with_local());
    registry.register(mem);
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(100)), 2);

    let info = run(
        &q,
        Operation::Move {
            sources: vec![loc(&src)],
            destination: Location::new("mem", "/dst"),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(
        !src.exists(),
        "the source must be gone after a successful move"
    );
    let mem_fs = q.registry().get("mem").unwrap();
    assert!(
        mem_fs.open_read("/dst/src/a.txt").await.is_ok(),
        "the file must be on the destination"
    );
}

/// OPS-REN-001 — renaming onto an existing name is refused and both survive.
#[tokio::test]
async fn rename_to_existing_name_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.txt");
    let b = tmp.path().join("b.txt");
    write(&a, "a");
    write(&b, "b");

    let info = run(
        &queue(),
        Operation::Rename {
            location: loc(&a),
            new_name: "b.txt".into(),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "alreadyExists");
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
}

/// OPS-REN-002 — a name with a path separator or empty is `invalid`.
#[tokio::test]
async fn rename_to_invalid_name() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.txt");
    write(&a, "a");
    for bad in ["x/y", ""] {
        let info = run(
            &queue(),
            Operation::Rename {
                location: loc(&a),
                new_name: bad.into(),
            },
        )
        .await;
        assert_eq!(info.status, OperationStatus::Failed, "{bad:?}");
        assert_eq!(info.error.unwrap().kind, "invalid", "{bad:?}");
    }
    assert!(a.exists());
}

/// OPS-TRASH-002 — trashing on a backend without a Trash is `unsupported`.
#[tokio::test]
async fn trash_on_remote_is_unsupported() {
    let mem = MemFs::with_default_id();
    mem.seed_file("/keep.txt", "keep");
    let mem: Arc<dyn dualpal_core::FileSystem> = Arc::new(mem);
    let registry = Arc::new(Registry::with_local());
    registry.register(mem);
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(100)), 2);

    let info = run(
        &q,
        Operation::Trash {
            targets: vec![Location::new("mem", "/keep.txt")],
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "unsupported");
    let mem_fs = q.registry().get("mem").unwrap();
    assert!(
        mem_fs.exists("/keep.txt").await.unwrap(),
        "nothing may be removed"
    );
}

/// OPS-Q-002 — cancelling a queued operation marks it cancelled and it never runs.
#[tokio::test]
async fn cancel_a_queued_operation() {
    let tmp = tempfile::tempdir().unwrap();
    let q = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(10)),
        1,
    );
    // First op occupies the only slot; the second stays queued.
    let first = q.enqueue(Operation::CreateDir {
        location: loc(&tmp.path().join("first")),
    });
    let second = q.enqueue(Operation::CreateDir {
        location: loc(&tmp.path().join("second")),
    });
    assert!(q.cancel(second), "a queued operation can be cancelled");
    let info = q.wait(second).await.unwrap();
    assert_eq!(info.status, OperationStatus::Cancelled);
    assert!(!tmp.path().join("second").exists(), "it never ran");
    assert!(q.wait(first).await.unwrap().status.is_finished());
}

/// OPS-Q-005 — every finished operation is recorded once, with its outcome.
#[tokio::test]
async fn every_finished_operation_is_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    // One slot so a queued operation can be cancelled before it ever runs.
    let q = OperationQueue::new(
        Arc::new(Registry::with_local()),
        Arc::new(ActivityLog::in_memory(100)),
        1,
    );

    // A completed copy.
    let good = tmp.path().join("good");
    write(&good, "x");
    let dst = tmp.path().join("dst");
    std::fs::create_dir(&dst).unwrap();
    run(
        &q,
        Operation::Copy {
            sources: vec![loc(&good)],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;

    // A failed delete.
    run(
        &q,
        Operation::Delete {
            targets: vec![loc(&tmp.path().join("missing"))],
        },
    )
    .await;

    // A cancelled operation: it stays queued behind a slow copy, then we
    // cancel it before it ever starts.
    let big = tmp.path().join("big.bin");
    std::fs::write(&big, vec![7u8; 16 * 1024 * 1024]).unwrap();
    let dst2 = tmp.path().join("dst2");
    std::fs::create_dir(&dst2).unwrap();
    let slow = q.enqueue(Operation::Copy {
        sources: vec![loc(&big)],
        destination: loc(&dst2),
        conflict: ConflictPolicy::Fail,
    });
    let cancelled = q.enqueue(Operation::CreateDir {
        location: loc(&tmp.path().join("will-cancel")),
    });
    assert!(q.cancel(cancelled), "a queued operation can be cancelled");
    assert_eq!(
        q.wait(cancelled).await.unwrap().status,
        OperationStatus::Cancelled
    );
    q.wait(slow).await.unwrap();

    let log = q.activity().list();
    assert!(log.len() >= 3, "{log:?}");
    let statuses: Vec<_> = log.iter().map(|r| r.status).collect();
    assert!(statuses.contains(&OperationStatus::Completed));
    assert!(statuses.contains(&OperationStatus::Failed));
    assert!(statuses.contains(&OperationStatus::Cancelled));
    assert!(log.iter().any(|r| r.kind == "copy" && r.bytes_done == 1));
    // The cancelled operation was recorded with a status, not silently dropped.
    assert!(log
        .iter()
        .any(|r| r.kind == "createDir" && r.status == OperationStatus::Cancelled));
}

/// OPS-Q-007 — the `OperationInfo` JSON shape the UI receives.
#[test]
fn operation_info_json_shape() {
    use dualpal_core::error::ErrorInfo;
    use dualpal_core::Progress;
    let info = dualpal_core::OperationInfo {
        id: uuid::Uuid::nil(),
        kind: "copy".into(),
        summary: "Copy 1 item to /dst".into(),
        status: OperationStatus::Completed,
        progress: Progress {
            files_total: 1,
            files_done: 1,
            bytes_total: 3,
            bytes_done: 3,
            skipped: 0,
            current: None,
        },
        error: None,
        queued_ms: 1_700_000_000_000,
        started_ms: Some(1_700_000_000_010),
        finished_ms: Some(1_700_000_000_020),
        places: vec!["local".into()],
    };
    insta::assert_json_snapshot!(info, @r###"
    {
      "id": "00000000-0000-0000-0000-000000000000",
      "kind": "copy",
      "summary": "Copy 1 item to /dst",
      "status": "completed",
      "progress": {
        "filesTotal": 1,
        "filesDone": 1,
        "bytesTotal": 3,
        "bytesDone": 3,
        "skipped": 0,
        "current": null
      },
      "error": null,
      "queuedMs": 1700000000000,
      "startedMs": 1700000000010,
      "finishedMs": 1700000000020,
      "places": [
        "local"
      ]
    }
    "###);

    // The error shape is part of the same contract.
    let failed = dualpal_core::OperationInfo {
        error: Some(ErrorInfo {
            kind: "alreadyExists".into(),
            message: "already exists: /x".into(),
        }),
        ..info
    };
    insta::assert_json_snapshot!(failed.error.unwrap(), @r###"
    {
      "kind": "alreadyExists",
      "message": "already exists: /x"
    }
    "###);
}

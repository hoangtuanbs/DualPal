//! Copy edge cases with injected failures (Phase 1, §8.3).

use std::sync::Arc;

use dualpal_core::vfs::LocalFs;
use dualpal_core::{
    ActivityLog, ConflictPolicy, Location, Operation, OperationQueue, OperationStatus, Registry,
};
use dualpal_testkit::{hash, FaultyFs};

fn setup(remote: &Arc<FaultyFs<LocalFs>>) -> OperationQueue {
    let registry = Registry::with_local();
    registry.register(remote.clone());
    OperationQueue::new(Arc::new(registry), Arc::new(ActivityLog::in_memory(100)), 2)
}

fn copy(src: &str, dst: &str, conflict: ConflictPolicy) -> Operation {
    Operation::Copy {
        sources: vec![Location::new("local", src)],
        destination: Location::new("remote", dst),
        conflict,
    }
}

fn data(len: usize) -> Vec<u8> {
    dualpal_testkit::fixture::pseudo_random(len, 7)
}

/// OPS-COPY-017 — a backend without resumable writes starts the file over.
#[tokio::test]
async fn non_resumable_backend_restarts_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("big.bin");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    let len = 8 * 1024 * 1024;
    std::fs::write(&src, data(len)).unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_resumable(false);
    remote.set_atomic_writes(false);
    remote.fail_write_after(3 * 1024 * 1024 + 123); // drop the first attempt midway

    let q = setup(&remote);
    let id = q.enqueue(copy(
        &src.to_string_lossy(),
        &dst.to_string_lossy(),
        ConflictPolicy::Fail,
    ));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    let got = std::fs::read(dst.join("big.bin")).unwrap();
    assert_eq!(hash(&got), hash(&data(len)));
    // The retry started from 0 rather than resuming.
    assert!(
        remote.resumed_offsets().is_empty(),
        "no open_write_at calls"
    );
}

/// OPS-COPY-022 — a full disk fails quickly with an `io` error and no retry loop.
#[tokio::test]
async fn full_disk_fails_without_retrying() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("doc.txt");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::write(dst.join("doc.txt"), "old").unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.break_writes();

    let q = setup(&remote);
    let start = std::time::Instant::now();
    let id = q.enqueue(copy(
        &src.to_string_lossy(),
        &dst.to_string_lossy(),
        ConflictPolicy::Overwrite,
    ));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "io");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "must not retry for long"
    );
    // The old file is untouched and no temporary file is left behind.
    assert_eq!(std::fs::read_to_string(dst.join("doc.txt")).unwrap(), "old");
    let leftovers: Vec<_> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("dualpal"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// OPS-COPY-020 — overwriting works even when the backend's rename refuses to
/// replace an existing file: the old file is moved aside, then removed.
#[tokio::test]
async fn overwrite_works_when_rename_refuses_to_replace() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("doc.txt");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::write(dst.join("doc.txt"), "old").unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_rename_replaces(false);

    let q = setup(&remote);
    let id = q.enqueue(copy(
        &src.to_string_lossy(),
        &dst.to_string_lossy(),
        ConflictPolicy::Overwrite,
    ));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(std::fs::read_to_string(dst.join("doc.txt")).unwrap(), "new");
    let names: Vec<_> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["doc.txt"], "no temporary files are left");
}

/// OPS-COPY-020 — when the new file cannot be renamed into place after the
/// old one was moved aside, the old one is put back and nothing is left.
#[tokio::test]
async fn overwrite_restores_the_old_file_when_the_rename_fails() {
    use dualpal_core::Error;
    use dualpal_testkit::FaultOp;

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("doc.txt");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::write(dst.join("doc.txt"), "old").unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_rename_replaces(false);
    // Renames: part over doc.txt (refused), doc.txt aside (ok), part to
    // doc.txt (fails here), aside back to doc.txt.
    remote.fail_later(
        FaultOp::Rename,
        2,
        Error::PermissionDenied("doc.txt".into()),
    );
    let q = setup(&remote);
    let id = q.enqueue(copy(
        &src.to_string_lossy(),
        &dst.to_string_lossy(),
        ConflictPolicy::Overwrite,
    ));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "permissionDenied");
    assert_eq!(std::fs::read_to_string(dst.join("doc.txt")).unwrap(), "old");
    let names: Vec<_> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["doc.txt"], "no temporary files are left");
}

/// OPS-COPY-020 — if the old file cannot be put back either, the error
/// names the hidden name it was left under, and it is still there.
#[tokio::test]
async fn overwrite_reports_where_the_old_file_was_left() {
    use dualpal_core::Error;
    use dualpal_testkit::FaultOp;

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("doc.txt");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::write(dst.join("doc.txt"), "old").unwrap();

    let remote = Arc::new(FaultyFs::with_id(LocalFs::new(), "remote"));
    remote.set_rename_replaces(false);
    // The third rename (part to doc.txt) fails, then so does the fourth
    // (aside back to doc.txt).
    remote.fail_later(
        FaultOp::Rename,
        2,
        Error::PermissionDenied("doc.txt".into()),
    );
    remote.fail_later(FaultOp::Rename, 0, Error::Connection("lost".into()));
    let q = setup(&remote);
    let id = q.enqueue(copy(
        &src.to_string_lossy(),
        &dst.to_string_lossy(),
        ConflictPolicy::Overwrite,
    ));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Failed);
    let err = info.error.unwrap();
    assert_eq!(err.kind, "permissionDenied");
    let aside: Vec<_> = std::fs::read_dir(&dst)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "old");
    let name = aside[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(err.message.contains(&name), "{}", err.message);
    assert!(err.message.contains("lost"), "{}", err.message);
}

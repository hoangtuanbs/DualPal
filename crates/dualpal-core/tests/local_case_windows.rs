//! Local-disk behaviours that only show on Windows or on case-insensitive
//! disks (Phase 1, §8.2 VFS-LOC-002/005/006 and §8.10 REN-013).

#![cfg(any(windows, target_os = "macos"))]

use std::path::Path;
use std::sync::Arc;

use dualpal_core::rename::RenameItem;
use dualpal_core::vfs::LocalFs;
use dualpal_core::{
    ActivityLog, ConflictPolicy, FileSystem, Location, Operation, OperationQueue, OperationStatus,
    Registry,
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

async fn run(q: &OperationQueue, op: Operation) -> dualpal_core::OperationInfo {
    let id = q.enqueue(op);
    q.wait(id).await.expect("operation known")
}

/// Whether the disk under `dir` ignores case (the default on Windows and
/// macOS, but either can be set up otherwise).
fn case_insensitive(dir: &Path) -> bool {
    let probe = dir.join("case-probe");
    std::fs::write(&probe, "x").unwrap();
    let insensitive = dir.join("CASE-PROBE").exists();
    std::fs::remove_file(&probe).unwrap();
    insensitive
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// VFS-LOC-006 — on a case-insensitive disk, renaming `a.txt` to `A.txt`
/// succeeds and the listing shows the new case.
#[tokio::test]
async fn case_only_rename_on_the_backend() {
    let tmp = tempfile::tempdir().unwrap();
    if !case_insensitive(tmp.path()) {
        eprintln!("skipped: this disk is case-sensitive");
        return;
    }
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    let fs = LocalFs::new();
    let dir = tmp.path().to_string_lossy().into_owned();
    fs.rename(&fs.join(&dir, "a.txt"), &fs.join(&dir, "A.txt"))
        .await
        .unwrap();
    let listed: Vec<String> = fs
        .list(&dir)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(listed, ["A.txt"]);

    // The single-item rename operation allows the same.
    let info = run(
        &queue(),
        Operation::Rename {
            location: loc(&tmp.path().join("A.txt")),
            new_name: "a.TXT".into(),
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(names(tmp.path()), ["a.TXT"]);
}

/// REN-013 — Multi Rename can change only the case of names on a
/// case-insensitive disk, files and folders alike.
#[tokio::test]
async fn multi_rename_changes_only_case() {
    let tmp = tempfile::tempdir().unwrap();
    if !case_insensitive(tmp.path()) {
        eprintln!("skipped: this disk is case-sensitive");
        return;
    }
    std::fs::write(tmp.path().join("report.txt"), "r").unwrap();
    std::fs::write(tmp.path().join("Photo.JPG"), "p").unwrap();
    std::fs::create_dir(tmp.path().join("folder")).unwrap();

    let items = [
        ("report.txt", "REPORT.txt"),
        ("Photo.JPG", "photo.jpg"),
        ("folder", "Folder"),
    ]
    .iter()
    .map(|(from, to)| RenameItem {
        location: loc(&tmp.path().join(from)),
        new_name: (*to).into(),
    })
    .collect();
    let info = run(&queue(), Operation::BatchRename { items }).await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(names(tmp.path()), ["Folder", "REPORT.txt", "photo.jpg"]);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("REPORT.txt")).unwrap(),
        "r"
    );
    // No temporary names are left behind.
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 3);
}

/// VFS-LOC-002 — a file with the Windows hidden attribute is hidden, even
/// without a leading dot.
#[cfg(windows)]
#[tokio::test]
async fn windows_hidden_attribute_counts_as_hidden() {
    let tmp = tempfile::tempdir().unwrap();
    let secret = tmp.path().join("secret.txt");
    let plain = tmp.path().join("plain.txt");
    std::fs::write(&secret, "s").unwrap();
    std::fs::write(&plain, "p").unwrap();
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&secret)
        .status()
        .expect("attrib runs");
    assert!(status.success());

    let fs = LocalFs::new();
    let listed = fs.list(&tmp.path().to_string_lossy()).await.unwrap();
    let hidden = |name: &str| listed.iter().find(|e| e.name == name).unwrap().hidden;
    assert!(hidden("secret.txt"), "attrib +h file must be hidden");
    assert!(!hidden("plain.txt"));
    assert!(fs.stat(&secret.to_string_lossy()).await.unwrap().hidden);
}

/// VFS-LOC-005 — drive roots and UNC shares have no parent and join plainly.
#[cfg(windows)]
#[test]
fn windows_drive_roots_and_unc_paths() {
    let fs = LocalFs::new();
    assert_eq!(fs.parent("C:\\"), None);
    assert_eq!(fs.join("C:\\", "x"), "C:\\x");
    assert_eq!(fs.parent("C:\\x").as_deref(), Some("C:\\"));
    assert_eq!(fs.parent("\\\\server\\share\\"), None);
    assert_eq!(fs.join("\\\\server\\share", "x"), "\\\\server\\share\\x");
    assert_eq!(
        fs.parent("\\\\server\\share\\x").as_deref(),
        Some("\\\\server\\share\\")
    );
    assert_eq!(
        fs.file_name("C:\\x\\notes.txt").as_deref(),
        Some("notes.txt")
    );
}

/// VFS-LOC-005 — paths longer than 260 characters work when written in the
/// `\\?\` form: they can be listed and copied.
#[cfg(windows)]
#[tokio::test]
async fn windows_long_paths_list_and_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let base = format!("\\\\?\\{}", tmp.path().display());
    let segment = "a".repeat(60);
    let mut deep = std::path::PathBuf::from(&base);
    for _ in 0..5 {
        deep.push(&segment);
    }
    std::fs::create_dir_all(&deep).unwrap();
    let file = deep.join("long-path-file.txt");
    std::fs::write(&file, "deep").unwrap();
    assert!(file.to_string_lossy().len() > 260);

    let fs = LocalFs::new();
    let listed = fs.list(&deep.to_string_lossy()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "long-path-file.txt");
    assert_eq!(listed[0].size, 4);

    let dst = std::path::PathBuf::from(&base).join("dst");
    std::fs::create_dir(&dst).unwrap();
    let info = run(
        &queue(),
        Operation::Copy {
            sources: vec![loc(&std::path::PathBuf::from(&base).join(&segment))],
            destination: loc(&dst),
            conflict: ConflictPolicy::Fail,
        },
    )
    .await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    let mut copied = dst.clone();
    for _ in 0..5 {
        copied.push(&segment);
    }
    assert_eq!(
        std::fs::read_to_string(copied.join("long-path-file.txt")).unwrap(),
        "deep"
    );
}

//! Folder size walks (`dualpal_core::folder_size`), on the local disk and on
//! an in-memory file system to show that the walk only uses the trait.

use std::sync::Arc;

use dualpal_core::folder_size::{calculate, FolderSize};
use dualpal_core::vfs::LocalFs;
use dualpal_core::{CancellationToken, Location, Registry};
use dualpal_testkit::MemFs;

fn local_registry() -> Registry {
    Registry::with_local()
}

/// A folder with a file at the top, a nested folder tree and an empty folder.
#[tokio::test]
async fn sums_bytes_files_and_folders_on_local_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("top.txt"), "abc").unwrap(); // 3 bytes
    std::fs::create_dir_all(root.join("a/sub")).unwrap();
    std::fs::write(root.join("a/x.txt"), "0123456789").unwrap(); // 10 bytes
    std::fs::write(root.join("a/sub/y.txt"), "hello").unwrap(); // 5 bytes
    std::fs::create_dir(root.join("empty")).unwrap();

    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let size = calculate(&local_registry(), &loc, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        size,
        FolderSize {
            bytes: 18,
            files: 3,
            folders: 3, // a, a/sub, empty: not the folder itself
            skipped: 0,
        }
    );
}

/// An empty folder has nothing in it.
#[tokio::test]
async fn empty_folder_is_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let size = calculate(&local_registry(), &loc, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(size, FolderSize::default());
}

/// A symlink back up the tree is not followed: the walk ends, and the link
/// counts as one item with no bytes.
#[cfg(unix)]
#[tokio::test]
async fn symlink_loop_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(root.join("inner")).unwrap();
    std::fs::write(root.join("inner/data.txt"), "four").unwrap(); // 4 bytes
    std::os::unix::fs::symlink(&root, root.join("inner/back")).unwrap();
    std::os::unix::fs::symlink(&root, root.join("up")).unwrap();

    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let size = calculate(&local_registry(), &loc, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        size,
        FolderSize {
            bytes: 4,
            files: 3,   // data.txt, inner/back and up (links count as items)
            folders: 1, // inner
            skipped: 0,
        }
    );
}

/// A subfolder that cannot be read is counted as skipped and the rest is still
/// added up. Permission bits are not enforced for root, so there the folder
/// reads fine and the test only checks the walk.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_subfolder_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("kept.txt"), "12").unwrap(); // 2 bytes
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("hidden.txt"), "secret").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let result = calculate(&local_registry(), &loc, &CancellationToken::new()).await;
    // Restore so the temp dir can be removed.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    let size = result.unwrap();
    if size.skipped == 0 {
        eprintln!("skipped: permission bits are not enforced here");
        assert_eq!(size.bytes, 2 + 6);
    } else {
        assert_eq!(
            size,
            FolderSize {
                bytes: 2,
                files: 1,
                folders: 1,
                skipped: 1,
            }
        );
    }
}

/// The same walk over the in-memory file system, which has a symlink loop too.
#[tokio::test]
async fn works_on_any_file_system() {
    let mem = MemFs::new("mem");
    mem.seed_dir("/d");
    mem.seed_file("/d/a.txt", "hello"); // 5 bytes
    mem.seed_dir("/d/sub");
    mem.seed_file("/d/sub/b.txt", "xy"); // 2 bytes
    mem.seed_symlink("/d/loop", "/d");
    let registry = Registry::new();
    registry.register(Arc::new(mem));

    let loc = Location::new("mem", "/d");
    let size = calculate(&registry, &loc, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        size,
        FolderSize {
            bytes: 7,
            files: 3,   // a.txt, b.txt and the loop link
            folders: 1, // sub
            skipped: 0,
        }
    );
}

/// Only folders can be totalled, and a missing one is reported as such.
#[tokio::test]
async fn file_and_missing_paths_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plain.txt");
    std::fs::write(&file, "x").unwrap();
    let registry = local_registry();
    let cancel = CancellationToken::new();

    let err = calculate(
        &registry,
        &Location::new(LocalFs::ID, file.to_string_lossy()),
        &cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "invalid");

    let missing = tmp.path().join("nope");
    let err = calculate(
        &registry,
        &Location::new(LocalFs::ID, missing.to_string_lossy()),
        &cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "notFound");
}

/// A walk whose token is already cancelled stops before reading anything.
#[tokio::test]
async fn cancelled_walk_stops() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "a").unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let err = calculate(&local_registry(), &loc, &cancel)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "cancelled");
}

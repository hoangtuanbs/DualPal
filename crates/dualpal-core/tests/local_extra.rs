//! Local-disk-only behaviours (Phase 1, §8.2 VFS-LOC).

#[cfg(unix)]
use dualpal_core::vfs::LocalFs;
#[cfg(unix)]
use dualpal_core::FileSystem;

/// VFS-LOC-001 — a read-only file is reported as `readonly`.
#[cfg(unix)]
#[tokio::test]
async fn readonly_flag_reflects_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("readonly.txt");
    std::fs::write(&p, "x").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();

    let fs = LocalFs::new();
    let e = fs.stat(&p.to_string_lossy()).await.unwrap();
    assert!(e.readonly, "a 0444 file must be readonly");
}

/// VFS-LOC-004 — a permission denied maps to the `permissionDenied` kind.
#[cfg(unix)]
#[tokio::test]
async fn permission_denied_maps_to_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("locked");
    std::fs::create_dir(&d).unwrap();
    std::fs::write(d.join("x.txt"), "x").unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o000)).unwrap();

    let fs = LocalFs::new();
    let result = fs.list(&d.to_string_lossy()).await;
    // Restore so the temp dir can be removed.
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();

    match result {
        Ok(_) => {
            // Running as root (or on a filesystem that ignores modes).
            eprintln!("skipped: permission bits are not enforced here");
        }
        Err(e) => assert_eq!(e.kind(), "permissionDenied"),
    }
}

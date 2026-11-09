//! The VFS contract suite against the local disk, plus the local-only path
//! cases from §8.2.

use std::sync::Arc;

use dualpal_core::vfs::LocalFs;
use dualpal_core::FileSystem;
use dualpal_testkit::{run_contract, Caps};

/// VFS-CON-030 — every `Error` variant serialises as `{kind, message}`.
#[test]
fn errors_serialise_as_json() {
    dualpal_testkit::contract::errors_serialise();
}

#[tokio::test]
async fn local_disk_passes_the_vfs_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new());
    let base = tmp.path().join("contract").to_string_lossy().into_owned();
    run_contract(fs, &base, Caps::local()).await;
}

/// VFS-PATH-002 — sanitising a name always yields a safe name.
#[test]
fn safe_local_name_never_unsafe() {
    proptest::proptest!(|(s in ".*")| {
        let safe = dualpal_core::vfs::safe_local_name(&s);
        assert!(dualpal_core::vfs::is_safe_local_name(&safe), "safe_local_name({s:?}) = {safe:?}");
    });
}

/// VFS-CON-024 — the path laws hold for generated names too.
#[test]
fn path_laws_for_generated_names() {
    let fs = LocalFs::new();
    let dir = "/proptest-root";
    proptest::proptest!(|(name in "[a-zA-Z0-9_\\-]{1,40}")| {
        let joined = fs.join(dir, &name);
        assert_eq!(fs.parent(&joined).as_deref(), Some(dir));
        assert_eq!(fs.file_name(&joined).as_deref(), Some(name.as_str()));
        assert!(fs.is_within(&joined, dir));
        assert!(!fs.is_within(dir, &joined));
    });
}

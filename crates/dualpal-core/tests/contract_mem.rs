//! The VFS contract suite against the in-memory backends: `MemFs` and
//! OpenDAL's in-memory S3 service. Both are fast and need no server.

use std::sync::Arc;

use dualpal_core::vfs::S3Fs;
use dualpal_core::FileSystem;
use dualpal_testkit::{run_contract, Caps, MemFs};

#[tokio::test]
async fn memfs_passes_the_vfs_contract() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemFs::with_default_id());
    run_contract(fs, "/contract", Caps::mem()).await;
}

#[tokio::test]
async fn opendal_memory_s3_passes_the_vfs_contract() {
    let op = opendal::Operator::new(opendal::services::Memory::default())
        .unwrap()
        .finish();
    let fs: Arc<dyn FileSystem> =
        Arc::new(S3Fs::from_operator("conn:mem".into(), "Memory".into(), op));
    run_contract(fs, "/contract", Caps::s3_memory()).await;
}

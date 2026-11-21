//! Files are written under a temporary name and put in place once complete.

use dualpal_core::vfs::{write_file_atomic, FileSystem};
use dualpal_testkit::{FaultyFs, MemFs};

fn parts(mem: &MemFs) -> Vec<String> {
    mem.paths()
        .into_iter()
        .filter(|p| p.contains("dualpal-part"))
        .collect()
}

/// VFS-CON-015 — a write that fails part way leaves the old file whole and
/// no temporary file behind.
#[tokio::test]
async fn failed_write_keeps_the_old_file() {
    let mem = MemFs::new("mem");
    mem.seed_file("/f.txt", "old contents");
    let fs = FaultyFs::new(mem.clone());
    fs.fail_write_after(3);
    assert!(write_file_atomic(&fs, "/f.txt", b"new contents")
        .await
        .is_err());
    assert_eq!(mem.contents("/f.txt").unwrap(), b"old contents");
    assert_eq!(parts(&mem), Vec::<String>::new());
}

/// VFS-CON-015 — on a backend whose rename will not replace a file, the
/// old one is moved aside and the new one still takes its place.
#[tokio::test]
async fn replaces_where_rename_does_not() {
    let mem = MemFs::new("mem");
    mem.seed_file("/f.txt", "old contents");
    let fs = FaultyFs::new(mem.clone());
    fs.set_rename_replaces(false);
    write_file_atomic(&fs, "/f.txt", b"new contents")
        .await
        .unwrap();
    assert_eq!(mem.contents("/f.txt").unwrap(), b"new contents");
    assert_eq!(parts(&mem), Vec::<String>::new());
}

/// VFS-CON-015 — a new file is written too, and a replaced one keeps its
/// permission bits.
#[tokio::test]
async fn writes_new_files_and_keeps_modes() {
    let mem = MemFs::new("mem");
    mem.seed_file("/old.sh", "echo old");
    mem.set_mode("/old.sh", 0o750).await.unwrap();
    write_file_atomic(&mem, "/new.txt", b"hello").await.unwrap();
    write_file_atomic(&mem, "/old.sh", b"echo new")
        .await
        .unwrap();
    assert_eq!(mem.contents("/new.txt").unwrap(), b"hello");
    assert_eq!(mem.contents("/old.sh").unwrap(), b"echo new");
    assert_eq!(
        mem.stat("/old.sh").await.unwrap().mode.map(|m| m & 0o777),
        Some(0o750)
    );
    assert_eq!(parts(&mem), Vec::<String>::new());
}

//! Deterministic folder trees and small helpers.
//!
//! Every tree is created with explicit modification times so that tests never
//! have to sleep to make one file newer than another.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 2024-01-02T03:04:05Z: the base modification time of the standard tree.
pub const BASE_TIME_MS: i64 = 1_704_164_645_000;

/// A tiny but valid 1×1 PNG, so image sniffing in the UI has something real.
const TINY_PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, b'I', b'D', b'A', b'T', 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xae,
    0x42, 0x60, 0x82,
];

/// Bytes that differ at every position, so a misplaced piece shows.
pub fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state & 0xff) as u8
        })
        .collect()
}

pub fn write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("create fixture folder");
    }
    std::fs::write(path, content).expect("write fixture file");
}

pub fn read(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path).expect("read fixture file")
}

pub fn set_mtime(path: impl AsRef<Path>, ms: i64) {
    let t = SystemTime::UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
    filetime::set_file_mtime(path.as_ref(), filetime::FileTime::from_system_time(t))
        .expect("set modification time");
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod fixture");
}

#[cfg(not(unix))]
fn set_mode(path: &Path, _mode: u32) {
    // Read-only is the only bit Windows has.
    let mut p = std::fs::metadata(path).expect("stat fixture").permissions();
    p.set_readonly(_mode & 0o200 == 0);
    std::fs::set_permissions(path, p).expect("chmod fixture");
}

fn symlink_dir(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

/// The realistic tree from the test plan. Returns `root`.
///
/// The 8 MiB `big.bin` makes this slower than [`small_tree`]; use the small
/// tree for tests that do not need the full variety.
pub fn standard_tree(root: impl AsRef<Path>) -> PathBuf {
    let root = root.as_ref().to_path_buf();
    std::fs::create_dir_all(&root).expect("create fixture root");
    let at = |ms: i64| BASE_TIME_MS + ms;

    let readme = root.join("docs/readme.md");
    write(&readme, "# Readme\n");
    set_mtime(&readme, at(0));

    let report = root.join("docs/report.pdf");
    write(&report, pseudo_random(2048, 7));
    set_mtime(&report, at(60_000));

    let notes = root.join("docs/notes.txt");
    write(&notes, "alpha\nbeta\n");
    set_mtime(&notes, at(120_000));

    for name in ["a.jpg", "b.jpg"] {
        let p = root.join("photos").join(name);
        write(&p, pseudo_random(64 * 1024, name.len() as u64));
        set_mtime(&p, at(180_000));
    }
    let png = root.join("photos/c.png");
    write(&png, TINY_PNG);
    set_mtime(&png, at(180_000));

    let ds = root.join("photos/.DS_Store");
    write(&ds, "junk");
    set_mtime(&ds, at(180_000));

    let deep = root.join("nested/l1/l2/l3/l4/deep.txt");
    write(&deep, "deep");
    set_mtime(&deep, at(240_000));

    std::fs::create_dir_all(root.join("empty-dir")).expect("empty dir");
    let empty = root.join("empty-file.txt");
    write(&empty, b"");
    set_mtime(&empty, at(300_000));

    let big = root.join("big.bin");
    write(&big, pseudo_random(8 * 1024 * 1024, 42));
    set_mtime(&big, at(360_000));

    let space = root.join("Space Name.txt");
    write(&space, "spaces");
    set_mtime(&space, at(420_000));

    let unicode = root.join("ünïcødé-文件.txt");
    write(&unicode, "unicode");
    set_mtime(&unicode, at(480_000));

    let case = root.join("Case.txt");
    write(&case, "upper");
    set_mtime(&case, at(540_000));
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        let lower = root.join("case.txt");
        write(&lower, "lower");
        set_mtime(&lower, at(600_000));
    }

    let hidden = root.join(".hidden-file");
    write(&hidden, "hidden");
    set_mtime(&hidden, at(660_000));

    let readonly = root.join("readonly.txt");
    write(&readonly, "read only");
    set_mtime(&readonly, at(720_000));
    set_mode(&readonly, 0o444);

    let script = root.join("script.sh");
    write(&script, "#!/bin/sh\necho hi\n");
    set_mtime(&script, at(780_000));
    set_mode(&script, 0o755);

    let _ = symlink_dir(&root.join("docs"), &root.join("link-to-docs"));

    root
}

/// Three files and one folder: for tests that only need "a realistic folder".
pub fn small_tree(root: impl AsRef<Path>) -> PathBuf {
    let root = root.as_ref().to_path_buf();
    let a = root.join("a.txt");
    write(&a, "alpha");
    set_mtime(&a, BASE_TIME_MS);
    let b = root.join("sub/b.txt");
    write(&b, "bravo!");
    set_mtime(&b, BASE_TIME_MS + 1000);
    let c = root.join("sub/deep/c.txt");
    write(&c, "charlie");
    set_mtime(&c, BASE_TIME_MS + 2000);
    root
}

/// `n` flat files in `root`.
pub fn many_files(root: impl AsRef<Path>, n: usize) -> PathBuf {
    let root = root.as_ref().to_path_buf();
    std::fs::create_dir_all(&root).expect("create many-files root");
    for i in 0..n {
        write(root.join(format!("file{i:05}.txt")), format!("{i}"));
    }
    root
}

/// Names that break naive path and URL handling.
pub fn name_torture() -> Vec<String> {
    let mut names = vec![
        "hash#tag.txt".to_string(),
        "percent%20name.txt".to_string(),
        "amp&more.txt".to_string(),
        "quote's.txt".to_string(),
        "brackets[1].txt".to_string(),
        "emoji-😀.txt".to_string(),
        "-leading-dash.txt".to_string(),
        "trailing-space .txt".to_string(),
        "two  spaces.txt".to_string(),
        "semi;colon.txt".to_string(),
        "plus+sign.txt".to_string(),
        "equals=sign.txt".to_string(),
        "at@sign.txt".to_string(),
        "question?.txt".to_string(),
    ];
    // A name at the usual 255-byte limit.
    names.push(format!("{}.txt", "x".repeat(251)));
    names
}

/// Hex SHA-256 of `bytes`, for comparing file contents.
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

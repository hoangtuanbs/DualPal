//! The VFS contract suite.
//!
//! Every backend (local disk, memory fake, SFTP, FTP, WebDAV, S3) must behave
//! the same way here. `run_contract` runs all cases; each case is also a
//! public function so a backend-specific test can call just one, and so the
//! remote suite can parametrise them with `rstest`.

use std::sync::Arc;

use dualpal_core::vfs::{EntryKind, FileSystem};
use dualpal_core::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::fixture::{hash, name_torture, pseudo_random};

/// What a backend can do. Cases that need a missing feature print a note and
/// return instead of failing.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub modes: bool,
    pub set_mtime: bool,
    pub symlinks: bool,
    pub trash: bool,
    pub resumable: bool,
    pub atomic_writes: bool,
    pub empty_dirs: bool,
    pub case_sensitive: bool,
}

impl Caps {
    pub fn local() -> Self {
        Self {
            modes: cfg!(unix),
            set_mtime: true,
            symlinks: cfg!(unix),
            trash: false,
            resumable: true,
            atomic_writes: false,
            empty_dirs: true,
            case_sensitive: !cfg!(any(windows, target_os = "macos")),
        }
    }

    pub fn mem() -> Self {
        Self {
            modes: true,
            set_mtime: true,
            symlinks: false,
            trash: false,
            resumable: true,
            atomic_writes: false,
            empty_dirs: true,
            case_sensitive: true,
        }
    }

    /// OpenDAL's in-memory S3: server-stamped times, no modes or symlinks.
    pub fn s3_memory() -> Self {
        Self {
            modes: false,
            set_mtime: false,
            symlinks: false,
            trash: false,
            resumable: false,
            atomic_writes: true,
            empty_dirs: true,
            case_sensitive: true,
        }
    }
}

async fn write_bytes(fs: &Arc<dyn FileSystem>, path: &str, data: &[u8]) -> Result<()> {
    let mut w = fs.open_write(path).await?;
    w.write_all(data).await.map_err(|e| Error::io(path, e))?;
    w.shutdown().await.map_err(|e| Error::io(path, e))
}

async fn read_bytes(fs: &Arc<dyn FileSystem>, path: &str) -> Result<Vec<u8>> {
    let mut r = fs.open_read(path).await?;
    let mut out = Vec::new();
    r.read_to_end(&mut out)
        .await
        .map_err(|e| Error::io(path, e))?;
    Ok(out)
}

/// A fresh subfolder for one case.
async fn case_dir(fs: &Arc<dyn FileSystem>, base: &str, name: &str) -> String {
    let dir = fs.join(base, name);
    match fs.create_dir(&dir).await {
        Ok(()) | Err(Error::AlreadyExists(_)) => {}
        Err(e) => panic!("creating {dir}: {e}"),
    }
    dir
}

/// Run every contract case against `fs` inside `base`.
pub async fn run_contract(fs: Arc<dyn FileSystem>, base: &str, caps: Caps) {
    if !fs.exists(base).await.unwrap_or(false) {
        fs.create_dir(base)
            .await
            .unwrap_or_else(|e| panic!("creating contract base {base}: {e}"));
    }
    list_empty_folder(&fs, base).await;
    create_list_stat_file(&fs, base).await;
    stat_folder_is_dir(&fs, base).await;
    stat_missing_is_not_found(&fs, base).await;
    exists_true_and_false(&fs, base).await;
    read_back_exact_bytes(&fs, base).await;
    open_write_truncates(&fs, base).await;
    create_dir_on_existing(&fs, base).await;
    create_dir_with_missing_parent(&fs, base).await;
    remove_file_case(&fs, base).await;
    remove_empty_dir(&fs, base).await;
    remove_non_empty_dir_fails(&fs, base).await;
    rename_file(&fs, base).await;
    rename_folder_with_contents(&fs, base).await;
    rename_onto_existing(&fs, base).await;
    hidden_detection(&fs, base).await;
    modified_time_is_set(&fs, base).await;
    set_modified_round_trip(&fs, base, caps).await;
    set_mode_round_trip(&fs, base, caps).await;
    open_read_at_offset(&fs, base).await;
    open_write_at_resumes(&fs, base, caps).await;
    name_torture_case(&fs, base).await;
    unicode_names(&fs, base).await;
    path_laws(fs.as_ref(), base);
    listing_1000_entries(&fs, base).await;
    concurrent_writers(&fs, base).await;
    symlink_reported(&fs, base, caps).await;
    empty_folders_survive(&fs, base, caps).await;
    display_name_and_id(&fs, base).await;
}

// ---- cases -----------------------------------------------------------------

/// VFS-CON-001
pub async fn list_empty_folder(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "001-empty").await;
    assert!(
        fs.list(&d).await.unwrap().is_empty(),
        "a new folder must be empty"
    );
}

/// VFS-CON-002
pub async fn create_list_stat_file(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "002-file").await;
    let path = fs.join(&d, "five.txt");
    write_bytes(fs, &path, b"hello").await.unwrap();
    let list = fs.list(&d).await.unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].name, "five.txt");
    assert_eq!(list[0].path, path);
    let st = fs.stat(&path).await.unwrap();
    assert_eq!(st.kind, EntryKind::File);
    assert_eq!(st.size, 5);
    assert_eq!(st.name, "five.txt");
    assert_eq!(st.path, path);
    assert_eq!(fs.file_name(&path).as_deref(), Some("five.txt"));
}

/// VFS-CON-003
pub async fn stat_folder_is_dir(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "003-dir").await;
    assert!(fs.stat(&d).await.unwrap().is_dir());
}

/// VFS-CON-004
pub async fn stat_missing_is_not_found(fs: &Arc<dyn FileSystem>, base: &str) {
    let missing = fs.join(base, "004-missing.txt");
    match fs.stat(&missing).await {
        Err(Error::NotFound(_)) => {}
        other => panic!("stat of a missing file: {other:?}"),
    }
    let under_missing = fs.join(&fs.join(base, "004-no-parent"), "file.txt");
    match fs.stat(&under_missing).await {
        Err(Error::NotFound(_)) => {}
        other => panic!("stat below a missing folder: {other:?}"),
    }
}

/// VFS-CON-005
pub async fn exists_true_and_false(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "005-exists").await;
    let path = fs.join(&d, "here.txt");
    write_bytes(fs, &path, b"x").await.unwrap();
    assert!(fs.exists(&path).await.unwrap());
    assert!(!fs.exists(&fs.join(&d, "nope.txt")).await.unwrap());
}

/// VFS-CON-006
pub async fn read_back_exact_bytes(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "006-bytes").await;
    for (name, data) in [
        ("empty.bin", Vec::new()),
        ("one.bin", vec![0xAB]),
        ("big.bin", pseudo_random(8 * 1024 * 1024, 6)),
    ] {
        let path = fs.join(&d, name);
        write_bytes(fs, &path, &data).await.unwrap();
        let got = read_bytes(fs, &path).await.unwrap();
        assert_eq!(hash(&got), hash(&data), "{name} came back different");
        assert_eq!(fs.stat(&path).await.unwrap().size, data.len() as u64);
    }
}

/// VFS-CON-007
pub async fn open_write_truncates(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "007-truncate").await;
    let path = fs.join(&d, "t.txt");
    write_bytes(fs, &path, b"long content").await.unwrap();
    write_bytes(fs, &path, b"x").await.unwrap();
    assert_eq!(read_bytes(fs, &path).await.unwrap(), b"x");
}

/// VFS-CON-008
pub async fn create_dir_on_existing(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "008-existing").await;
    match fs.create_dir(&d).await {
        Err(Error::AlreadyExists(_)) => {}
        other => panic!("creating an existing folder: {other:?}"),
    }
}

/// VFS-CON-009
///
/// Backends differ today: a missing parent may be an error or may be
/// created. Both are accepted here; the plan asks for one behaviour later.
pub async fn create_dir_with_missing_parent(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "009-parent").await;
    let parent = fs.join(&d, "not-there");
    let child = fs.join(&parent, "child");
    match fs.create_dir(&child).await {
        Err(Error::NotFound(_)) => {}
        Ok(()) => {
            eprintln!("note: backend creates missing parent folders");
            let _ = fs.remove_dir(&child).await;
            let _ = fs.remove_dir(&parent).await;
        }
        Err(e) => panic!("creating a folder below a missing parent: {e}"),
    }
}

/// VFS-CON-010
pub async fn remove_file_case(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "010-remove-file").await;
    let path = fs.join(&d, "gone.txt");
    write_bytes(fs, &path, b"bye").await.unwrap();
    fs.remove_file(&path).await.unwrap();
    assert!(fs.list(&d).await.unwrap().is_empty());
    assert!(matches!(fs.stat(&path).await, Err(Error::NotFound(_))));
}

/// VFS-CON-011
pub async fn remove_empty_dir(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "011-remove-dir").await;
    fs.remove_dir(&d).await.unwrap();
    assert!(!fs.exists(&d).await.unwrap());
}

/// VFS-CON-012
pub async fn remove_non_empty_dir_fails(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "012-non-empty").await;
    let child = fs.join(&d, "child.txt");
    write_bytes(fs, &child, b"still here").await.unwrap();
    assert!(
        fs.remove_dir(&d).await.is_err(),
        "removing a non-empty folder must fail"
    );
    assert!(fs.exists(&child).await.unwrap(), "child was deleted");
}

/// VFS-CON-013
pub async fn rename_file(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "013-rename-file").await;
    let from = fs.join(&d, "before.txt");
    let to = fs.join(&d, "after.txt");
    write_bytes(fs, &from, b"content").await.unwrap();
    fs.rename(&from, &to).await.unwrap();
    assert!(!fs.exists(&from).await.unwrap());
    assert_eq!(read_bytes(fs, &to).await.unwrap(), b"content");
}

/// VFS-CON-014
pub async fn rename_folder_with_contents(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "014-rename-dir").await;
    let from = fs.join(&d, "old");
    fs.create_dir(&from).await.unwrap();
    write_bytes(fs, &fs.join(&from, "a.txt"), b"a")
        .await
        .unwrap();
    fs.create_dir(&fs.join(&from, "sub")).await.unwrap();
    write_bytes(fs, &fs.join(&fs.join(&from, "sub"), "b.txt"), b"b")
        .await
        .unwrap();
    let to = fs.join(&d, "new");
    fs.rename(&from, &to).await.unwrap();
    assert!(!fs.exists(&from).await.unwrap());
    assert_eq!(read_bytes(fs, &fs.join(&to, "a.txt")).await.unwrap(), b"a");
    assert_eq!(
        read_bytes(fs, &fs.join(&fs.join(&to, "sub"), "b.txt"))
            .await
            .unwrap(),
        b"b"
    );
}

/// VFS-CON-015
///
/// Backends may refuse or replace; both are accepted, but a failure must not
/// destroy either file. The operation queue never relies on replace-by-rename
/// (see OPS-COPY-020).
pub async fn rename_onto_existing(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "015-rename-existing").await;
    let from = fs.join(&d, "from.txt");
    let to = fs.join(&d, "to.txt");
    write_bytes(fs, &from, b"new").await.unwrap();
    write_bytes(fs, &to, b"old").await.unwrap();
    match fs.rename(&from, &to).await {
        Ok(()) => assert_eq!(read_bytes(fs, &to).await.unwrap(), b"new"),
        Err(Error::AlreadyExists(_)) | Err(Error::Unsupported(_)) | Err(Error::Io { .. }) => {
            assert!(fs.exists(&from).await.unwrap(), "source disappeared");
            assert_eq!(read_bytes(fs, &to).await.unwrap(), b"old");
        }
        Err(e) => panic!("renaming onto an existing name: {e}"),
    }
}

/// VFS-CON-016
pub async fn hidden_detection(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "016-hidden").await;
    let path = fs.join(&d, ".hidden");
    write_bytes(fs, &path, b"h").await.unwrap();
    assert!(fs.stat(&path).await.unwrap().hidden);
    let visible = fs.join(&d, "shown.txt");
    write_bytes(fs, &visible, b"v").await.unwrap();
    assert!(!fs.stat(&visible).await.unwrap().hidden);
}

/// VFS-CON-017
pub async fn modified_time_is_set(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "017-mtime").await;
    let path = fs.join(&d, "now.txt");
    write_bytes(fs, &path, b"x").await.unwrap();
    let Some(ms) = fs.stat(&path).await.unwrap().modified_ms else {
        eprintln!("note: backend reports no modification time");
        return;
    };
    let delta = (dualpal_core::now_ms() - ms).abs();
    assert!(delta < 5 * 60 * 1000, "mtime is {delta} ms from now");
}

/// VFS-CON-018
pub async fn set_modified_round_trip(fs: &Arc<dyn FileSystem>, base: &str, caps: Caps) {
    if !caps.set_mtime {
        eprintln!("skipped: backend cannot set modification times");
        return;
    }
    let d = case_dir(fs, base, "018-set-mtime").await;
    let path = fs.join(&d, "stamped.txt");
    write_bytes(fs, &path, b"x").await.unwrap();
    let when = 1_600_000_000_000;
    fs.set_modified(&path, when).await.unwrap();
    let got = fs.stat(&path).await.unwrap().modified_ms.unwrap();
    assert!(
        (got - when).abs() <= 2000,
        "set_modified stored {got}, wanted {when}"
    );
}

/// VFS-CON-019
pub async fn set_mode_round_trip(fs: &Arc<dyn FileSystem>, base: &str, caps: Caps) {
    let d = case_dir(fs, base, "019-mode").await;
    let path = fs.join(&d, "mode.txt");
    write_bytes(fs, &path, b"x").await.unwrap();
    if !caps.modes {
        assert!(
            matches!(fs.set_mode(&path, 0o640).await, Err(Error::Unsupported(_))),
            "a backend without modes must say unsupported"
        );
        return;
    }
    fs.set_mode(&path, 0o640).await.unwrap();
    assert_eq!(
        fs.stat(&path).await.unwrap().mode.map(|m| m & 0o777),
        Some(0o640)
    );
}

/// VFS-CON-020
pub async fn open_read_at_offset(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "020-read-at").await;
    let path = fs.join(&d, "abcdef.txt");
    write_bytes(fs, &path, b"abcdef").await.unwrap();
    let mut r = fs.open_read_at(&path, 3).await.unwrap();
    let mut rest = String::new();
    r.read_to_string(&mut rest).await.unwrap();
    assert_eq!(rest, "def");
    match fs.open_read_at(&path, 7).await {
        Err(Error::Invalid(_)) => {}
        Err(e) => panic!("reading past the end: {e}"),
        Ok(_) => panic!("reading past the end must be invalid"),
    }
}

/// VFS-CON-021
pub async fn open_write_at_resumes(fs: &Arc<dyn FileSystem>, base: &str, caps: Caps) {
    let d = case_dir(fs, base, "021-resume").await;
    let path = fs.join(&d, "resumed.bin");
    let data = pseudo_random(1024 * 1024 + 123, 21);
    let half = data.len() / 2;
    write_bytes(fs, &path, &data[..half]).await.unwrap();
    if !caps.resumable {
        assert!(
            matches!(
                fs.open_write_at(&path, half as u64).await,
                Err(Error::Unsupported(_))
            ),
            "a non-resumable backend must say unsupported"
        );
        return;
    }
    let mut w = fs.open_write_at(&path, half as u64).await.unwrap();
    w.write_all(&data[half..]).await.unwrap();
    w.shutdown().await.unwrap();
    let got = read_bytes(fs, &path).await.unwrap();
    assert_eq!(hash(&got), hash(&data), "resumed file differs");
}

/// VFS-CON-022
pub async fn name_torture_case(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "022-names").await;
    for (i, name) in name_torture().into_iter().enumerate() {
        let path = fs.join(&d, &name);
        if write_bytes(fs, &path, name.as_bytes()).await.is_err() {
            eprintln!("note: this backend does not allow the name {name:?}");
            continue;
        }
        let names: Vec<String> = fs
            .list(&d)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert!(
            names.contains(&name),
            "listing does not contain {name:?}: {names:?}"
        );
        assert_eq!(read_bytes(fs, &path).await.unwrap(), name.as_bytes());
        let renamed = fs.join(&d, &format!("renamed-{i}.txt"));
        fs.rename(&path, &renamed)
            .await
            .unwrap_or_else(|e| panic!("renaming {name:?}: {e}"));
        fs.remove_file(&renamed).await.unwrap();
    }
}

/// VFS-CON-023
pub async fn unicode_names(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "023-unicode").await;
    let nfc = "café-文件.txt";
    let nfd = "cafe\u{301}-文件.txt";
    let path = fs.join(&d, nfc);
    write_bytes(fs, &path, b"unicode").await.unwrap();
    // macOS may store the decomposed form; stat must accept what was written.
    let st = fs
        .stat(&path)
        .await
        .unwrap_or_else(|e| panic!("stat of {nfc:?} after writing it: {e}"));
    assert_eq!(st.kind, EntryKind::File);
    let listed = fs.list(&d).await.unwrap();
    let found = listed.iter().any(|e| e.name == nfc || e.name == nfd);
    assert!(found, "listing has no form of the name: {listed:?}");
}

/// VFS-CON-024
///
/// The laws hold for every name a backend accepts. `contract_local.rs` also
/// runs this through proptest with generated names.
pub fn path_laws(fs: &dyn FileSystem, dir: &str) {
    let names = [
        "a.txt",
        "with space.txt",
        "ünïcødé.txt",
        ".hidden",
        "dash-name",
        "under_score",
        "UPPER.TXT",
    ];
    for name in names {
        let joined = fs.join(dir, name);
        assert_eq!(
            fs.parent(&joined).as_deref(),
            Some(dir),
            "parent of {joined:?}"
        );
        assert_eq!(
            fs.file_name(&joined).as_deref(),
            Some(name),
            "file_name of {joined:?}"
        );
        assert!(fs.is_within(&joined, dir), "{joined:?} within {dir:?}");
        assert!(!fs.is_within(dir, &joined), "{dir:?} within {joined:?}");
    }
    assert!(!fs.is_within("/a/bc", "/a/b"));
}

/// VFS-CON-025
pub async fn listing_1000_entries(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "025-many").await;
    for i in 0..1000 {
        let path = fs.join(&d, &format!("f{i:04}.txt"));
        write_bytes(fs, &path, b"").await.unwrap();
    }
    let listed = fs.list(&d).await.unwrap();
    assert_eq!(listed.len(), 1000, "listing is incomplete");
}

/// VFS-CON-026
pub async fn concurrent_writers(fs: &Arc<dyn FileSystem>, base: &str) {
    let d = case_dir(fs, base, "026-parallel").await;
    let workers = fs.parallel_transfers().clamp(1, 4);
    let mut set = tokio::task::JoinSet::new();
    for i in 0..workers {
        let fs = fs.clone();
        let path = fs.join(&d, &format!("w{i}.bin"));
        let data = pseudo_random(256 * 1024, 100 + i as u64);
        set.spawn(async move {
            write_bytes(&fs, &path, &data).await.unwrap();
            (path, data)
        });
    }
    while let Some(done) = set.join_next().await {
        let (path, data) = done.unwrap();
        let got = read_bytes(fs, &path).await.unwrap();
        assert_eq!(hash(&got), hash(&data), "{path} differs");
    }
}

/// VFS-CON-027
pub async fn symlink_reported(fs: &Arc<dyn FileSystem>, base: &str, caps: Caps) {
    if !caps.symlinks {
        eprintln!("skipped: backend has no symlinks");
        return;
    }
    #[cfg(unix)]
    {
        let d = case_dir(fs, base, "027-symlink").await;
        let target = fs.join(&d, "target.txt");
        write_bytes(fs, &target, b"target").await.unwrap();
        let link = fs.join(&d, "link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let e = fs.stat(&link).await.unwrap();
        assert!(e.symlink);
        assert_eq!(e.kind, EntryKind::File);
        let dangling = fs.join(&d, "dangling");
        std::os::unix::fs::symlink(fs.join(&d, "missing"), &dangling).unwrap();
        let e = fs.stat(&dangling).await.unwrap();
        assert!(e.symlink);
        assert_eq!(e.kind, EntryKind::Other);
        assert!(fs.list(&d).await.is_ok(), "listing a dangling link failed");
    }
    #[cfg(not(unix))]
    {
        let _ = (fs, base);
        eprintln!("skipped: symlink creation is not available here");
    }
}

/// VFS-CON-028
pub async fn empty_folders_survive(fs: &Arc<dyn FileSystem>, base: &str, caps: Caps) {
    if !caps.empty_dirs {
        eprintln!("skipped: backend cannot store empty folders");
        return;
    }
    let d = case_dir(fs, base, "028-empty-dir").await;
    let sub = fs.join(&d, "empty-sub");
    fs.create_dir(&sub).await.unwrap();
    let names: Vec<String> = fs
        .list(&d)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"empty-sub".to_string()), "{names:?}");
    fs.remove_dir(&sub).await.unwrap();
    let names: Vec<String> = fs
        .list(&d)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(!names.contains(&"empty-sub".to_string()), "{names:?}");
}

/// VFS-CON-029
pub async fn display_name_and_id(fs: &Arc<dyn FileSystem>, base: &str) {
    assert!(!fs.id().is_empty());
    assert_eq!(fs.id(), fs.id(), "the id changed between calls");
    assert!(!fs.display_name().is_empty());
    let d = case_dir(fs, base, "029-id").await;
    let path = fs.join(&d, "stable.txt");
    write_bytes(fs, &path, b"x").await.unwrap();
    let a = fs.stat(&path).await.unwrap();
    let b = fs.stat(&path).await.unwrap();
    assert_eq!(a.path, b.path);
    assert_eq!(a.name, b.name);
    assert_eq!(fs.file_name(&a.path).as_deref(), Some(a.name.as_str()));
}

/// VFS-CON-030
///
/// Run once per process (it is not backend-specific and uses an inline
/// snapshot), so it is not part of [`run_contract`].
pub fn errors_serialise() {
    use serde_json::json;
    let errors = vec![
        Error::NotFound("/x".into()),
        Error::AlreadyExists("/x".into()),
        Error::PermissionDenied("/x".into()),
        Error::UnknownFileSystem("nope".into()),
        Error::Invalid("bad".into()),
        Error::Unsupported("no".into()),
        Error::Cancelled,
        Error::Connection("down".into()),
        Error::AuthFailed("no".into()),
        Error::UnknownHost {
            host: "h".into(),
            fingerprint: "SHA256:x".into(),
        },
        Error::HostKeyChanged {
            host: "h".into(),
            fingerprint: "SHA256:x".into(),
        },
        Error::Io {
            path: "/x".into(),
            source: std::io::Error::other("boom"),
        },
    ];
    let values: Vec<serde_json::Value> = errors
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    assert_eq!(
        values[0],
        json!({"kind": "notFound", "message": "not found: /x"})
    );
    assert_eq!(
        values[6],
        json!({"kind": "cancelled", "message": "operation cancelled"})
    );
    assert_eq!(
        values[9],
        json!({
            "kind": "unknownHost",
            "message": "unknown host h, key fingerprint SHA256:x",
            "fingerprint": "SHA256:x"
        })
    );
    insta::assert_json_snapshot!(values, @r###"
[
  {
    "kind": "notFound",
    "message": "not found: /x"
  },
  {
    "kind": "alreadyExists",
    "message": "already exists: /x"
  },
  {
    "kind": "permissionDenied",
    "message": "permission denied: /x"
  },
  {
    "kind": "unknownFileSystem",
    "message": "unknown file system: nope"
  },
  {
    "kind": "invalid",
    "message": "invalid request: bad"
  },
  {
    "kind": "unsupported",
    "message": "not supported: no"
  },
  {
    "kind": "cancelled",
    "message": "operation cancelled"
  },
  {
    "kind": "connection",
    "message": "could not connect: down"
  },
  {
    "kind": "authFailed",
    "message": "sign-in failed: no"
  },
  {
    "fingerprint": "SHA256:x",
    "kind": "unknownHost",
    "message": "unknown host h, key fingerprint SHA256:x"
  },
  {
    "fingerprint": "SHA256:x",
    "kind": "hostKeyChanged",
    "message": "the host key of h has changed (now SHA256:x); this could be an attack"
  },
  {
    "kind": "io",
    "message": "I/O error on /x: boom"
  }
]
"###);
}

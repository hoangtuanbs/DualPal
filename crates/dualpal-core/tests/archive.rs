//! Archives as read-only folders (`dualpal_core::vfs::archive`): every
//! format is built in a temp folder, opened, listed and read, and copied out
//! through the operation queue.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dualpal_core::vfs::archive::{open_archive, Archives, ProgressFn, ENCRYPTED};
use dualpal_core::vfs::{EntryKind, FileSystem, LocalFs};
use dualpal_core::{
    CancellationToken, ConflictPolicy, Error, Location, Operation, OperationStatus, Registry,
};
use dualpal_testkit::MemFs;
use tokio::io::AsyncReadExt;

/// The files every test archive holds (folders are implied by the paths).
const FILES: &[(&str, &[u8])] = &[
    ("top.txt", b"top level"),
    ("docs/a.txt", b"alpha"),
    ("docs/deep/b.txt", b"bravo bravo"),
    ("docs/deep/empty.txt", b""),
];

fn no_progress() -> ProgressFn {
    Arc::new(|_| {})
}

fn make_zip(path: &Path, files: &[(&str, &[u8])], dirs: &[&str]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for d in dirs {
        w.add_directory(*d, opts).unwrap();
    }
    for (name, data) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

/// A tar written header by header, so that names are kept exactly as given
/// (tar::Builder refuses `..`).
fn tar_bytes(files: &[(&str, &[u8])], dirs: &[&str]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut put = |name: &str, data: &[u8], dir: bool| {
        let mut h = tar::Header::new_gnu();
        let bytes = name.as_bytes();
        h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        h.set_size(data.len() as u64);
        h.set_mode(if dir { 0o755 } else { 0o644 });
        h.set_mtime(1_600_000_000);
        h.set_entry_type(if dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        h.set_cksum();
        b.append(&h, data).unwrap();
    };
    for d in dirs {
        put(d, b"", true);
    }
    for (name, data) in files {
        put(name, data, false);
    }
    b.into_inner().unwrap()
}

fn make_7z(path: &Path, files: &[(&str, &[u8])], dirs: &[&str]) {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let mut w = ArchiveWriter::new(std::fs::File::create(path).unwrap()).unwrap();
    for d in dirs {
        w.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory(d), None)
            .unwrap();
    }
    for (name, data) in files {
        let entry = ArchiveEntry::new_file(name);
        if data.is_empty() {
            w.push_archive_entry::<&[u8]>(entry, None).unwrap();
        } else {
            w.push_archive_entry(entry, Some(*data)).unwrap();
        }
    }
    w.finish().unwrap();
}

/// Write an archive called `name` holding `files` into `dir`; the format
/// follows the name.
fn make(dir: &Path, name: &str, files: &[(&str, &[u8])], dirs: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let tar = || tar_bytes(files, dirs);
    if name.ends_with(".zip") {
        make_zip(&path, files, dirs);
    } else if name.ends_with(".7z") {
        make_7z(&path, files, dirs);
    } else if name.ends_with(".tar") {
        std::fs::write(&path, tar()).unwrap();
    } else if name.ends_with(".tgz") || name.ends_with(".tar.gz") {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(&tar()).unwrap();
        std::fs::write(&path, e.finish().unwrap()).unwrap();
    } else if name.ends_with(".tar.bz2") {
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        e.write_all(&tar()).unwrap();
        std::fs::write(&path, e.finish().unwrap()).unwrap();
    } else if name.ends_with(".tar.xz") {
        let mut e =
            lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(6)).unwrap();
        e.write_all(&tar()).unwrap();
        std::fs::write(&path, e.finish().unwrap()).unwrap();
    } else {
        panic!("unknown format {name}");
    }
    path
}

const FORMATS: &[&str] = &[
    "t.zip",
    "t.tar",
    "t.tgz",
    "t.tar.gz",
    "t.tar.bz2",
    "t.tar.xz",
    "t.7z",
];

async fn open_local(
    registry: &Registry,
    path: &Path,
    scratch: &Path,
) -> dualpal_core::Result<Arc<dyn FileSystem>> {
    let loc = Location::new(LocalFs::ID, path.to_string_lossy());
    let fs = open_archive(
        registry,
        &loc,
        "archive:test".into(),
        scratch,
        &CancellationToken::new(),
        no_progress(),
    )
    .await?;
    Ok(Arc::new(fs))
}

async fn read_all(fs: &dyn FileSystem, path: &str) -> std::io::Result<Vec<u8>> {
    let mut r = fs.open_read(path).await.map_err(std::io::Error::other)?;
    let mut out = Vec::new();
    r.read_to_end(&mut out).await?;
    Ok(out)
}

fn names(entries: &[dualpal_core::Entry]) -> Vec<String> {
    let mut v: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    v.sort();
    v
}

/// VFS-ARC-001: every format lists nested folders and reads each file back.
#[tokio::test]
async fn lists_and_reads_every_format() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::with_local();
    for name in FORMATS {
        let path = make(tmp.path(), name, FILES, &["docs/"]);
        let fs = open_local(&registry, &path, &tmp.path().join("scratch"))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            names(&fs.list("/").await.unwrap()),
            ["docs", "top.txt"],
            "{name}"
        );
        assert_eq!(
            names(&fs.list("/docs").await.unwrap()),
            ["a.txt", "deep"],
            "{name}"
        );
        assert_eq!(
            names(&fs.list("/docs/deep").await.unwrap()),
            ["b.txt", "empty.txt"],
            "{name}"
        );
        let docs = fs.stat("/docs").await.unwrap();
        assert_eq!(docs.kind, EntryKind::Dir, "{name}");
        for (file, data) in FILES {
            let p = format!("/{file}");
            let e = fs.stat(&p).await.unwrap();
            assert_eq!(e.kind, EntryKind::File, "{name} {file}");
            assert_eq!(e.size, data.len() as u64, "{name} {file}");
            assert!(e.readonly);
            assert_eq!(
                read_all(fs.as_ref(), &p).await.unwrap(),
                *data,
                "{name} {file}"
            );
        }
        // Paths behave like a folder tree.
        assert_eq!(fs.parent("/docs/deep").as_deref(), Some("/docs"));
        assert_eq!(fs.parent("/docs").as_deref(), Some("/"));
        assert_eq!(fs.parent("/"), None);
        assert_eq!(fs.join("/", "docs"), "/docs");
        assert!(fs.is_within("/docs/a.txt", "/docs"));
        assert!(!fs.is_within("/docsx", "/docs"));
        assert_eq!(fs.file_name("/").as_deref(), Some("t"));
        assert!(matches!(fs.stat("/missing").await, Err(Error::NotFound(_))));
        assert!(fs.open_read("/docs").await.is_err());
    }
}

/// VFS-ARC-002: anything that would change an archive fails as read-only.
#[tokio::test]
async fn archives_are_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let path = make(tmp.path(), "r.zip", FILES, &[]);
    let fs = open_local(&Registry::with_local(), &path, tmp.path())
        .await
        .unwrap();
    let errors = [
        fs.create_dir("/new").await.err(),
        fs.remove_file("/top.txt").await.err(),
        fs.remove_dir("/docs").await.err(),
        fs.rename("/top.txt", "/x.txt").await.err(),
        fs.open_write("/top.txt").await.err(),
        fs.trash("/top.txt").await.err(),
        fs.set_mode("/top.txt", 0o600).await.err(),
    ];
    for e in errors {
        let e = e.expect("write refused");
        assert_eq!(e.kind(), "permissionDenied");
        assert!(e.to_string().contains("read-only"), "{e}");
    }
}

/// VFS-ARC-003: empty archives open as empty folders.
#[tokio::test]
async fn empty_archives_are_empty_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::with_local();
    for name in FORMATS {
        let path = make(tmp.path(), &format!("empty-{name}"), &[], &[]);
        let fs = open_local(&registry, &path, tmp.path())
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(fs.list("/").await.unwrap().is_empty(), "{name}");
    }
}

/// VFS-ARC-004: a damaged or fake archive gives a clear error, not a listing.
#[tokio::test]
async fn corrupt_archives_fail_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::with_local();
    // Not an archive at all, under every name.
    for name in FORMATS {
        let path = tmp.path().join(format!("fake-{name}"));
        std::fs::write(
            &path,
            b"this is not an archive, just some text that goes on",
        )
        .unwrap();
        let e = open_local(&registry, &path, tmp.path()).await.err();
        let e = e.unwrap_or_else(|| panic!("{name} opened"));
        assert!(
            matches!(e, Error::Invalid(_) | Error::Unsupported(_)),
            "{name}: {e}"
        );
    }
    // A real archive cut in half.
    for name in ["cut.zip", "cut.7z", "cut.tgz"] {
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let path = make(tmp.path(), name, &[("big.bin", &big)], &[]);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let opened = open_local(&registry, &path, tmp.path()).await;
        match opened {
            Err(e) => assert!(matches!(e, Error::Invalid(_)), "{name}: {e}"),
            // A tar whose end is missing may list; its file must not read.
            Ok(fs) => assert!(read_all(fs.as_ref(), "/big.bin").await.is_err(), "{name}"),
        }
    }
}

/// VFS-ARC-005: `../` and absolute entry names stay inside the archive,
/// and copying them out never writes outside the destination.
#[tokio::test]
async fn traversal_names_stay_inside() {
    let tmp = tempfile::tempdir().unwrap();
    let evil: &[(&str, &[u8])] = &[
        ("../evil.txt", b"one"),
        ("/abs.txt", b"two"),
        ("a/../../../up.txt", b"three"),
    ];
    let registry = Arc::new(Registry::with_local());
    let queue = dualpal_testkit::queue(registry.clone());
    for name in ["evil.zip", "evil.tar"] {
        let path = make(tmp.path(), name, evil, &[]);
        let fs = open_local(&registry, &path, &tmp.path().join("scratch"))
            .await
            .unwrap();
        assert_eq!(
            names(&fs.list("/").await.unwrap()),
            ["abs.txt", "evil.txt", "up.txt"]
        );
        assert_eq!(read_all(fs.as_ref(), "/evil.txt").await.unwrap(), b"one");

        // Extract into out/inner: nothing may land in out/.
        let out = tmp.path().join(format!("out-{name}"));
        let inner = out.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        registry.register(fs.clone());
        let info = dualpal_testkit::queue::run(
            &queue,
            Operation::Copy {
                sources: vec![Location::new(fs.id(), "/")],
                destination: Location::new(LocalFs::ID, inner.to_string_lossy()),
                conflict: ConflictPolicy::Fail,
            },
        )
        .await;
        assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
        registry.unregister(fs.id());
        let mut top: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        top.sort();
        assert_eq!(top, ["inner"]);
        assert_eq!(std::fs::read(inner.join("evil/up.txt")).unwrap(), b"three");
        assert!(!tmp.path().join("evil.txt").exists());
    }
}

/// Flip the "encrypted" bit of every central directory record.
fn mark_encrypted(path: &Path) {
    let mut b = std::fs::read(path).unwrap();
    let mut i = 0;
    while i + 4 <= b.len() {
        if &b[i..i + 4] == b"PK\x01\x02" {
            b[i + 8] |= 1;
        }
        i += 1;
    }
    std::fs::write(path, b).unwrap();
}

/// VFS-ARC-006: password-protected archives say so.
#[tokio::test]
async fn encrypted_archives_are_named_as_such() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::with_local();
    let zip = make(tmp.path(), "locked.zip", FILES, &[]);
    mark_encrypted(&zip);
    let e = open_local(&registry, &zip, tmp.path()).await.err().unwrap();
    assert_eq!(e.kind(), "unsupported");
    assert!(e.to_string().contains(ENCRYPTED), "{e}");

    for encrypt_header in [true, false] {
        use sevenz_rust2::{encoder_options::AesEncoderOptions, ArchiveEntry, ArchiveWriter};
        use sevenz_rust2::{EncoderMethod, Password};
        let path = tmp.path().join(format!("locked-{encrypt_header}.7z"));
        let mut w = ArchiveWriter::new(std::fs::File::create(&path).unwrap()).unwrap();
        w.set_content_methods(vec![
            AesEncoderOptions::new(Password::from("secret")).into(),
            EncoderMethod::LZMA2.into(),
        ]);
        w.set_encrypt_header(encrypt_header);
        w.push_archive_entry(ArchiveEntry::new_file("x.txt"), Some(&b"hidden text"[..]))
            .unwrap();
        w.finish().unwrap();
        let e = open_local(&registry, &path, tmp.path())
            .await
            .err()
            .unwrap();
        assert!(
            e.to_string().contains(ENCRYPTED),
            "header {encrypt_header}: {e}"
        );
    }
}

/// VFS-ARC-007: zip entries sharing their data (an overlapping zip bomb)
/// are refused.
#[tokio::test]
async fn overlapping_zip_entries_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let path = make(
        tmp.path(),
        "bomb.zip",
        &[("a.txt", b"aaaa"), ("b.txt", b"bbbb")],
        &[],
    );
    // Point the second central record at the first local header.
    let mut b = std::fs::read(&path).unwrap();
    let records: Vec<usize> = (0..b.len() - 4)
        .filter(|&i| &b[i..i + 4] == b"PK\x01\x02")
        .collect();
    let first = b[records[0] + 42..records[0] + 46].to_vec();
    b[records[1] + 42..records[1] + 46].copy_from_slice(&first);
    std::fs::write(&path, b).unwrap();
    let e = open_local(&Registry::with_local(), &path, tmp.path())
        .await
        .err()
        .unwrap();
    assert!(e.to_string().contains("zip bomb"), "{e}");
}

/// VFS-ARC-007: an entry whose data, which starts after a long name in its
/// local header, runs into the next entry's header is refused too.
#[tokio::test]
async fn overlap_after_a_long_local_name_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default());
    let zip_with = |files: &[(&str, &[u8])]| {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    };
    // b.txt's local header and data, as the writer lays them out.
    let alone = zip_with(&[("b.txt", b"bbbb")]);
    let quoted = alone[..30 + 5 + 4].to_vec();
    assert_eq!(&quoted[..4], b"PK\x03\x04");
    // a.txt has a long name and holds that header as its data.
    let long = format!("{}.txt", "a".repeat(200));
    let mut b = zip_with(&[(&long, &quoted), ("b.txt", b"bbbb")]);
    let a_data = (30 + long.len()) as u32;
    // Point b.txt's central record into a.txt's data.
    let records: Vec<usize> = (0..b.len() - 4)
        .filter(|&i| &b[i..i + 4] == b"PK\x01\x02")
        .collect();
    b[records[1] + 42..records[1] + 46].copy_from_slice(&a_data.to_le_bytes());
    let path = tmp.path().join("long.zip");
    std::fs::write(&path, b).unwrap();
    let e = open_local(&Registry::with_local(), &path, tmp.path())
        .await
        .err()
        .expect("opened");
    assert!(e.to_string().contains("zip bomb"), "{e}");
}

/// VFS-ARC-008: copying out of an archive goes through the queue with
/// progress, and copying the archive's top makes a folder named after it.
#[tokio::test]
async fn extract_through_the_queue() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::with_local());
    let queue = dualpal_testkit::queue(registry.clone());
    let archives = Archives::new(registry.clone(), queue.clone(), tmp.path().join("scratch"));
    for name in FORMATS {
        let path = make(tmp.path(), name, FILES, &[]);
        let loc = Location::new(LocalFs::ID, path.to_string_lossy());
        let fs = archives
            .open(&loc, &CancellationToken::new(), no_progress())
            .await
            .unwrap();
        let out = tmp.path().join(format!("out-{name}"));
        std::fs::create_dir(&out).unwrap();
        let info = dualpal_testkit::queue::run(
            &queue,
            Operation::Copy {
                sources: vec![Location::new(fs.id(), "/")],
                destination: Location::new(LocalFs::ID, out.to_string_lossy()),
                conflict: ConflictPolicy::Fail,
            },
        )
        .await;
        assert_eq!(
            info.status,
            OperationStatus::Completed,
            "{name}: {:?}",
            info.error
        );
        assert_eq!(info.progress.files_done, FILES.len() as u64);
        assert_eq!(
            info.progress.bytes_done,
            FILES.iter().map(|(_, d)| d.len() as u64).sum::<u64>()
        );
        for (file, data) in FILES {
            assert_eq!(
                std::fs::read(out.join("t").join(file)).unwrap(),
                *data,
                "{name} {file}"
            );
        }
        // Moving out of an archive copies and then fails to delete.
        let moved = dualpal_testkit::queue::run(
            &queue,
            Operation::Move {
                sources: vec![Location::new(fs.id(), "/top.txt")],
                destination: Location::new(LocalFs::ID, out.to_string_lossy()),
                conflict: ConflictPolicy::Fail,
            },
        )
        .await;
        assert_eq!(moved.status, OperationStatus::Failed);
        assert!(fs.stat("/top.txt").await.is_ok());
        archives.close(fs.id());
    }
    assert!(archives.ids().is_empty());
}

/// VFS-ARC-009: an archive on a server is downloaded to the scratch folder,
/// and the scratch folder goes once the archive is closed and unused.
#[tokio::test]
async fn archives_on_servers_are_downloaded_and_cleaned_up() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::with_local());
    let server = MemFs::new("mem");
    let local = make(tmp.path(), "remote.tgz", FILES, &[]);
    server.seed_dir("/pub");
    server.seed_file("/pub/remote.tgz", std::fs::read(&local).unwrap());
    registry.register(Arc::new(server));
    let queue = dualpal_testkit::queue(registry.clone());
    let scratch = tmp.path().join("scratch");
    let archives = Archives::new(registry.clone(), queue, scratch.clone());

    let phases = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = phases.clone();
    let progress: ProgressFn = Arc::new(move |p| seen.lock().unwrap().push(p.phase));
    let fs = archives
        .open(
            &Location::new("mem", "/pub/remote.tgz"),
            &CancellationToken::new(),
            progress,
        )
        .await
        .unwrap();
    assert_eq!(fs.source(), &Location::new("mem", "/pub/remote.tgz"));
    assert_eq!(fs.display_name(), "remote.tgz");
    assert!(registry.get(fs.id()).is_ok());
    let phases = phases.lock().unwrap().clone();
    assert!(
        phases.contains(&"download") && phases.contains(&"unpack"),
        "{phases:?}"
    );
    assert_eq!(
        read_all(fs.as_ref(), "/docs/a.txt").await.unwrap(),
        b"alpha"
    );
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 1);

    // A reader still open keeps the files; dropping everything removes them.
    let reader = fs.open_read("/docs/deep/b.txt").await.unwrap();
    let id = fs.id().to_string();
    archives.close(&id);
    assert!(registry.get(&id).is_err());
    drop(fs);
    let mut reader = reader;
    let mut text = Vec::new();
    reader.read_to_end(&mut text).await.unwrap();
    assert_eq!(text, b"bravo bravo");
    drop(reader);
    dualpal_testkit::eventually(
        "scratch folder removed",
        std::time::Duration::from_secs(5),
        || {
            std::fs::read_dir(&scratch)
                .map(|d| d.count() == 0)
                .unwrap_or(true)
        },
    )
    .await;

    // Cancelling while opening stops it.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let e = archives
        .open(
            &Location::new("mem", "/pub/remote.tgz"),
            &cancel,
            no_progress(),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e}");
    archives.close_all();
    assert!(!scratch.exists());
}

/// VFS-ARC-010: an archive an operation still uses stays open until the
/// operation ends.
#[tokio::test]
async fn close_waits_for_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::with_local());
    // One slot, taken by a slow copy, so the extraction waits in the queue.
    let queue = dualpal_testkit::queue_with(
        registry.clone(),
        Arc::new(dualpal_core::ActivityLog::in_memory(10)),
        1,
    );
    let archives = Archives::new(registry.clone(), queue.clone(), tmp.path().join("scratch"));
    let path = make(tmp.path(), "w.zip", FILES, &[]);
    let fs = archives
        .open(
            &Location::new(LocalFs::ID, path.to_string_lossy()),
            &CancellationToken::new(),
            no_progress(),
        )
        .await
        .unwrap();
    let out = tmp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let id = queue.enqueue(Operation::Copy {
        sources: vec![Location::new(fs.id(), "/docs")],
        destination: Location::new(LocalFs::ID, out.to_string_lossy()),
        conflict: ConflictPolicy::Fail,
    });
    archives.close(fs.id());
    let info = queue.wait(id).await.unwrap();
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(std::fs::read(out.join("docs/a.txt")).unwrap(), b"alpha");
    archives.reap();
    assert!(registry.get(fs.id()).is_err());
    assert!(archives.ids().is_empty());
}

/// VFS-ARC-010: an extraction that stopped on an existing file keeps its
/// archive open after it is closed, so "Replace existing" can run, until
/// the finished operations are cleared.
#[tokio::test]
async fn failed_extraction_can_be_retried_after_close() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::with_local());
    let queue = dualpal_testkit::queue(registry.clone());
    let archives = Archives::new(registry.clone(), queue.clone(), tmp.path().join("scratch"));
    let path = make(tmp.path(), "pack.zip", FILES, &[]);
    let fs = archives
        .open(
            &Location::new(LocalFs::ID, path.to_string_lossy()),
            &CancellationToken::new(),
            no_progress(),
        )
        .await
        .unwrap();
    let id = fs.id().to_string();
    drop(fs);
    let out = tmp.path().join("out");
    std::fs::create_dir_all(out.join("pack")).unwrap();
    std::fs::write(out.join("pack/top.txt"), b"old").unwrap();
    let copy = |conflict| Operation::Copy {
        sources: vec![Location::new(&id, "/")],
        destination: Location::new(LocalFs::ID, out.to_string_lossy()),
        conflict,
    };
    let op = queue.enqueue(copy(ConflictPolicy::Fail));
    // Extract Here lets go of the archive right after queueing.
    archives.close(&id);
    let info = queue.wait(op).await.unwrap();
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.error.unwrap().kind, "alreadyExists");
    archives.reap();
    assert!(registry.get(&id).is_ok(), "closed before the retry");

    let info = dualpal_testkit::queue::run(&queue, copy(ConflictPolicy::Overwrite)).await;
    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(
        std::fs::read(out.join("pack/top.txt")).unwrap(),
        b"top level"
    );
    archives.reap();
    assert!(registry.get(&id).is_ok());

    queue.clear_finished();
    archives.reap();
    assert!(registry.get(&id).is_err());
    assert!(archives.ids().is_empty());
}

/// VFS-ARC-011: folder sizes and name search work inside an archive.
#[tokio::test]
async fn folder_size_and_find_work_inside() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(Registry::with_local());
    let path = make(tmp.path(), "s.7z", FILES, &[]);
    let fs = open_local(&registry, &path, tmp.path()).await.unwrap();
    registry.register(fs.clone());
    let size = dualpal_core::folder_size::calculate(
        &registry,
        &Location::new(fs.id(), "/docs"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!((size.bytes, size.files, size.folders), (16, 3, 1));
    let found = dualpal_core::search::find_by_name(
        &registry,
        &Location::new(fs.id(), "/"),
        "b.txt",
        dualpal_core::search::SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].path, "/docs/deep/b.txt");
}

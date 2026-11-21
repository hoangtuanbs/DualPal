//! Checksum calculation, verification and writing through the local file system.

use std::sync::{Arc, Mutex};

use dualpal_core::checksum::{self, Algorithm, VerifyStatus};
use dualpal_core::error::Error;
use dualpal_core::vfs::{Location, Registry};
use dualpal_core::CancellationToken;

fn local(dir: &std::path::Path) -> Location {
    Location::new("local", dir.to_string_lossy().to_string())
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";
const MD5_EMPTY: &str = "d41d8cd98f00b204e9800998ecf8427e";

#[tokio::test]
async fn calculate_walks_folders_recursively() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    std::fs::write(dir.join("a.txt"), "abc").unwrap();
    std::fs::write(dir.join("sub/b.txt"), "").unwrap();
    std::fs::write(dir.join("sub/deeper/c.txt"), "abc").unwrap();
    let registry = Registry::with_local();

    let last = Arc::new(Mutex::new(None));
    let seen = last.clone();
    let entries = checksum::calculate(
        &registry,
        &local(dir),
        &names(&["a.txt", "sub"]),
        Algorithm::Md5,
        &CancellationToken::new(),
        move |p| *seen.lock().unwrap() = Some(p),
    )
    .await
    .unwrap();

    let got: Vec<(&str, Option<&str>)> = entries
        .iter()
        .map(|e| (e.path.as_str(), e.hash.as_deref()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("a.txt", Some(MD5_ABC)),
            ("sub/b.txt", Some(MD5_EMPTY)),
            ("sub/deeper/c.txt", Some(MD5_ABC)),
        ]
    );
    let p = last.lock().unwrap().clone().unwrap();
    assert_eq!((p.files_done, p.files_total), (3, 3));
    assert_eq!((p.bytes_done, p.bytes_total), (6, 6));
}

#[tokio::test]
async fn calculate_streams_large_files_with_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(tmp.path().join("big.bin"), &data).unwrap();
    let registry = Registry::with_local();

    let steps = Arc::new(Mutex::new(Vec::new()));
    let seen = steps.clone();
    let entries = checksum::calculate(
        &registry,
        &local(tmp.path()),
        &names(&["big.bin"]),
        Algorithm::Sha256,
        &CancellationToken::new(),
        move |p| seen.lock().unwrap().push(p),
    )
    .await
    .unwrap();

    assert_eq!(
        entries[0].hash.as_deref(),
        Some(checksum::hash_bytes(Algorithm::Sha256, &data).as_str())
    );
    let steps = steps.lock().unwrap();
    // Several chunks, each reported, so the bar moves while a big file hashes.
    let byte_steps: Vec<u64> = steps.iter().map(|p| p.bytes_done).collect();
    assert!(byte_steps.windows(2).all(|w| w[0] <= w[1]));
    assert!(
        steps
            .iter()
            .filter(|p| p.bytes_done > 0 && p.bytes_done < data.len() as u64)
            .count()
            >= 2
    );
    assert_eq!(steps.last().unwrap().bytes_done, data.len() as u64);
}

#[tokio::test]
async fn calculate_stops_when_cancelled() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "abc").unwrap();
    let registry = Registry::with_local();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = checksum::calculate(
        &registry,
        &local(tmp.path()),
        &names(&["a.txt"]),
        Algorithm::Md5,
        &cancel,
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
}

#[tokio::test]
async fn verify_reports_ok_failed_and_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "abc").unwrap();
    std::fs::write(dir.join("sub/c.txt"), "changed").unwrap();
    let sums = format!(
        "{MD5_ABC}  a.txt\n{MD5_ABC}  sub/c.txt\n{MD5_EMPTY}  gone.txt\n{MD5_EMPTY} *sub\n"
    );
    std::fs::write(dir.join("SUMS.md5"), sums).unwrap();
    let registry = Registry::with_local();
    let file = Location::new("local", dir.join("SUMS.md5").to_string_lossy().to_string());

    let report = checksum::verify(
        &registry,
        &file,
        Algorithm::Md5,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!((report.ok, report.failed, report.missing), (1, 1, 2));
    let status: Vec<(&str, VerifyStatus)> = report
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e.status))
        .collect();
    assert_eq!(
        status,
        vec![
            ("a.txt", VerifyStatus::Ok),
            ("sub/c.txt", VerifyStatus::Failed),
            ("gone.txt", VerifyStatus::Missing),
            ("sub", VerifyStatus::Missing),
        ]
    );
    assert_eq!(
        report.entries[1].actual.as_deref(),
        Some(checksum::hash_bytes(Algorithm::Md5, b"changed").as_str())
    );
    assert_eq!(report.entries[2].actual, None);
}

#[tokio::test]
async fn verify_rejects_a_malformed_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("bad.md5"),
        format!("{MD5_ABC}  a\nnot a checksum\n"),
    )
    .unwrap();
    let registry = Registry::with_local();
    let file = Location::new(
        "local",
        tmp.path().join("bad.md5").to_string_lossy().to_string(),
    );
    let err = checksum::verify(
        &registry,
        &file,
        Algorithm::Md5,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "invalid");
    assert!(err.to_string().contains("line 2"), "{err}");
}

#[tokio::test]
async fn written_file_verifies_and_keeps_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("a.txt"), "abc").unwrap();
    std::fs::write(dir.join("b.txt"), "").unwrap();
    let registry = Registry::with_local();
    let folder = local(dir);
    let entries = checksum::calculate(
        &registry,
        &folder,
        &names(&["a.txt", "b.txt"]),
        Algorithm::Sha256,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();

    // The chosen extension replaces a checksum extension already on the name.
    let saved = checksum::write_file(
        &registry,
        &folder,
        "SUMS.md5",
        Algorithm::Sha256,
        &entries,
        false,
    )
    .await
    .unwrap();
    assert_eq!(saved.path, dir.join("SUMS.sha256").to_string_lossy());
    let report = checksum::verify(
        &registry,
        &saved,
        Algorithm::Sha256,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!((report.ok, report.failed, report.missing), (2, 0, 0));

    let again = checksum::write_file(
        &registry,
        &folder,
        "SUMS.sha256",
        Algorithm::Sha256,
        &entries,
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(again.kind(), "alreadyExists");
    checksum::write_file(
        &registry,
        &folder,
        "SUMS.sha256",
        Algorithm::Sha256,
        &entries[..1],
        true,
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(dir.join("SUMS.sha256")).unwrap();
    assert_eq!(text.lines().count(), 1);

    let bad = checksum::write_file(&registry, &folder, "a/b", Algorithm::Md5, &entries, true)
        .await
        .unwrap_err();
    assert_eq!(bad.kind(), "invalid");
}

#[tokio::test]
async fn unknown_checksum_file_is_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::with_local();
    let file = Location::new(
        "local",
        tmp.path().join("none.md5").to_string_lossy().to_string(),
    );
    let err = checksum::verify(
        &registry,
        &file,
        Algorithm::Md5,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "notFound");
}

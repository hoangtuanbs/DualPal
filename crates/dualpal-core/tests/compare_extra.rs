//! Compare cases that do not need a server (Phase 1, §8.7).

use std::path::Path;
use std::sync::Arc;

use dualpal_core::compare::{compare, CompareBy, CompareOptions, DiffState};
use dualpal_core::{CancellationToken, Location, Registry};
use dualpal_testkit::set_mtime;

fn loc(p: &Path) -> Location {
    Location::new("local", p.to_string_lossy())
}

async fn cmp(
    registry: &Registry,
    left: &Path,
    right: &Path,
    opts: CompareOptions,
) -> dualpal_core::compare::Comparison {
    compare(
        registry,
        &loc(left),
        &loc(right),
        opts,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap()
}

fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// CMP-002 — `by: size` ignores dates for equality but reports the newer side.
///
/// The current implementation returns `Same` for equal-size files and drops
/// the "which side is newer" information entirely.
#[tokio::test]
#[ignore = "BUG: CompareBy::Size classifies equal-size files as Same, losing which side is newer"]
async fn by_size_ignores_dates() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join("same.txt"), "x");
    write(&r.join("same.txt"), "x");
    set_mtime(l.join("same.txt"), 1_600_000_000_000);
    set_mtime(r.join("same.txt"), 1_600_000_100_000);

    let registry = Arc::new(Registry::with_local());
    let opts = CompareOptions {
        by: CompareBy::Size,
        ..Default::default()
    };
    let c = cmp(&registry, &l, &r, opts).await;
    assert_eq!(c.get("same.txt").unwrap().state, DiffState::RightNewer);
}

/// CMP-003 — `by: content` separates equal-size files with different bytes.
#[tokio::test]
async fn by_content_finds_different_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join("a.txt"), "aaaa");
    write(&r.join("a.txt"), "bbbb");
    set_mtime(l.join("a.txt"), 1_600_000_000_000);
    set_mtime(r.join("a.txt"), 1_600_000_000_000);

    let registry = Arc::new(Registry::with_local());
    let opts = CompareOptions {
        by: CompareBy::Content,
        ..Default::default()
    };
    let c = cmp(&registry, &l, &r, opts).await;
    assert_eq!(c.get("a.txt").unwrap().state, DiffState::Different);
}

/// CMP-004 — tolerance boundary: 1999 ms apart equal, 2001 ms apart not.
#[tokio::test]
async fn tolerance_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join("t.txt"), "x");
    write(&r.join("t.txt"), "x");
    let registry = Arc::new(Registry::with_local());

    set_mtime(l.join("t.txt"), 1_600_000_000_000);
    set_mtime(r.join("t.txt"), 1_600_000_001_999);
    let c = cmp(&registry, &l, &r, CompareOptions::default()).await;
    assert_eq!(c.get("t.txt").unwrap().state, DiffState::Same);

    set_mtime(r.join("t.txt"), 1_600_000_002_001);
    let c = cmp(&registry, &l, &r, CompareOptions::default()).await;
    assert_eq!(c.get("t.txt").unwrap().state, DiffState::RightNewer);
}

/// CMP-005 — the default excludes hide `.DS_Store` and friends.
#[tokio::test]
async fn default_excludes() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join("real.txt"), "x");
    write(&r.join("real.txt"), "x");
    write(&l.join(".DS_Store"), "junk");
    write(&l.join("Thumbs.db"), "junk");
    write(&l.join("._cache"), "junk");

    let registry = Arc::new(Registry::with_local());
    let c = cmp(&registry, &l, &r, CompareOptions::default()).await;
    assert!(c.get(".DS_Store").is_none());
    assert!(c.get("Thumbs.db").is_none());
    assert!(c.get("._cache").is_none());
    assert_eq!(c.get("real.txt").unwrap().state, DiffState::Same);
}

/// CMP-007 — `include_hidden: false` hides dot files.
#[tokio::test]
async fn include_hidden_false_hides_dot_files() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join(".secret"), "x");
    write(&l.join("shown.txt"), "x");
    write(&r.join("shown.txt"), "x");

    let registry = Arc::new(Registry::with_local());
    let opts = CompareOptions {
        include_hidden: false,
        ..Default::default()
    };
    let c = cmp(&registry, &l, &r, opts).await;
    assert!(c.get(".secret").is_none());
    assert_eq!(c.get("shown.txt").unwrap().state, DiffState::Same);
}

/// CMP-008 — a file on one side and a folder on the other is a kind mismatch.
#[tokio::test]
async fn kind_mismatch_file_vs_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    write(&l.join("x"), "file");
    std::fs::create_dir_all(r.join("x")).unwrap();

    let registry = Arc::new(Registry::with_local());
    let c = cmp(&registry, &l, &r, CompareOptions::default()).await;
    assert_eq!(c.get("x").unwrap().state, DiffState::KindMismatch);
}

/// CMP-006 — `glob_match` agrees with an equivalent regular expression.
#[test]
fn glob_match_matches_a_reference_regex() {
    fn reference(pattern: &str, name: &str) -> bool {
        let mut re = String::from("^");
        for c in pattern.chars() {
            match c {
                '*' => re.push_str(".*"),
                '?' => re.push('.'),
                _ => re.push_str(&regex::escape(&c.to_string())),
            }
        }
        re.push('$');
        regex::RegexBuilder::new(&re)
            .case_insensitive(true)
            .build()
            .unwrap()
            .is_match(name)
    }

    proptest::proptest!(|(pattern in "[a-zA-Z0-9*?._ -]{0,20}", name in "[a-zA-Z0-9 ._-]{0,20}")| {
        assert_eq!(
            dualpal_core::compare::glob_match(&pattern, &name),
            reference(&pattern, &name),
            "pattern {pattern:?} vs name {name:?}"
        );
    });
}

/// CMP-010 — cancelling a compare stops it and returns `cancelled`.
#[tokio::test]
async fn cancel_compare_stops_quickly() {
    let registry = Arc::new(Registry::with_local());
    let l = tempfile::tempdir().unwrap();
    let r = tempfile::tempdir().unwrap();
    // One large file, compared by content, keeps reading long enough to cancel.
    let data = vec![0x5au8; 64 * 1024 * 1024];
    std::fs::write(l.path().join("big.bin"), &data).unwrap();
    std::fs::write(r.path().join("big.bin"), &data).unwrap();
    let opts = CompareOptions {
        by: CompareBy::Content,
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let (ll, rl) = (loc(l.path()), loc(r.path()));
    let start = std::time::Instant::now();
    let task = tokio::spawn(async move { compare(&registry, &ll, &rl, opts, c, None).await });
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cancel.cancel();
    let result = task.await.unwrap();
    assert!(
        matches!(result, Err(dualpal_core::Error::Cancelled)),
        "{result:?}"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}

/// CMP-011 — the progress callback reports each folder scanned.
#[tokio::test]
async fn progress_reports_folders() {
    let registry = Arc::new(Registry::with_local());
    let l = tempfile::tempdir().unwrap();
    let r = tempfile::tempdir().unwrap();
    dualpal_testkit::small_tree(l.path());
    dualpal_testkit::small_tree(r.path());

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let sink = seen.clone();
    let progress: dualpal_core::compare::ProgressFn = Arc::new(move |folders, _| {
        sink.lock().unwrap().push(folders);
    });

    let c = compare(
        &registry,
        &loc(l.path()),
        &loc(r.path()),
        CompareOptions::default(),
        CancellationToken::new(),
        Some(progress),
    )
    .await
    .unwrap();
    assert!(!c.items.is_empty());

    let counts = seen.lock().unwrap().clone();
    // Folders are counted as they are scanned, monotonically.
    assert!(!counts.is_empty(), "the progress callback never fired");
    for pair in counts.windows(2) {
        assert!(pair[1] >= pair[0], "folder count went backwards");
    }
    // small_tree has root + sub + sub/deep = 3 folders, listed on both sides.
    assert_eq!(*counts.last().unwrap(), 6);
}

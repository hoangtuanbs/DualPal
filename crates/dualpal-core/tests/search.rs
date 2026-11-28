//! Name search (`dualpal_core::search`) on the local disk.

use dualpal_core::search::{find_by_name, name_matches, SearchOptions};
use dualpal_core::vfs::LocalFs;
use dualpal_core::{CancellationToken, Error, Location, Registry};

fn registry() -> Registry {
    Registry::with_local()
}

fn tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("Report.txt"), "a").unwrap();
    std::fs::create_dir_all(root.join("docs/deep")).unwrap();
    std::fs::write(root.join("docs/report-2025.md"), "b").unwrap();
    std::fs::write(root.join("docs/deep/REPORT_final.pdf"), "c").unwrap();
    std::fs::write(root.join("other.txt"), "d").unwrap();
    tmp
}

#[test]
fn name_match_is_case_insensitive_substring() {
    assert!(name_matches("Report.txt", "report"));
    assert!(name_matches("Report.txt", "  ort.T "));
    assert!(!name_matches("Report.txt", "pdf"));
    assert!(!name_matches("Report.txt", ""));
    assert!(!name_matches("Report.txt", "   "));
}

#[tokio::test]
async fn finds_matches_at_all_depths_shallowest_first() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let res = find_by_name(
        &registry(),
        &loc,
        "report",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let names: Vec<&str> = res.hits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Report.txt", "report-2025.md", "REPORT_final.pdf"]
    );
    assert!(!res.truncated);
    assert_eq!(res.skipped, 0);
}

#[tokio::test]
async fn stops_at_max_results_and_says_so() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let opts = SearchOptions {
        max_results: 2,
        max_depth: 32,
        ..Default::default()
    };
    let res = find_by_name(&registry(), &loc, "report", opts, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(res.hits.len(), 2);
    assert!(res.truncated);
}

#[tokio::test]
async fn reaching_the_cap_stops_without_reading_more_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("report-1.txt"), "a").unwrap();
    std::fs::write(root.join("report-2.txt"), "b").unwrap();
    std::fs::create_dir(root.join("empty-of-matches")).unwrap();
    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let opts = SearchOptions {
        max_results: 2,
        max_depth: 32,
        ..Default::default()
    };
    let res = find_by_name(&registry(), &loc, "report", opts, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(res.hits.len(), 2);
    assert!(res.truncated);
    assert_eq!(res.skipped, 0);
}

#[tokio::test]
async fn zero_cap_returns_truncated_at_once() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let opts = SearchOptions {
        max_results: 0,
        max_depth: 32,
        ..Default::default()
    };
    let res = find_by_name(&registry(), &loc, "report", opts, &CancellationToken::new())
        .await
        .unwrap();
    assert!(res.hits.is_empty());
    assert!(res.truncated);
}

#[tokio::test]
async fn max_depth_limits_descent() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let opts = SearchOptions {
        max_results: 100,
        max_depth: 1,
        ..Default::default()
    };
    let res = find_by_name(&registry(), &loc, "report", opts, &CancellationToken::new())
        .await
        .unwrap();
    let names: Vec<&str> = res.hits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Report.txt", "report-2025.md"]);
}

#[tokio::test]
async fn empty_query_returns_nothing() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let res = find_by_name(
        &registry(),
        &loc,
        " ",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(res.hits.is_empty());
}

#[tokio::test]
async fn missing_start_folder_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let loc = Location::new(LocalFs::ID, tmp.path().join("nope").to_string_lossy());
    let res = find_by_name(
        &registry(),
        &loc,
        "x",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn cancelled_search_returns_cancelled() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let res = find_by_name(
        &registry(),
        &loc,
        "report",
        SearchOptions::default(),
        &cancel,
    )
    .await;
    assert!(matches!(res, Err(Error::Cancelled)));
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_folders_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir_all(root.join("inner")).unwrap();
    std::fs::write(root.join("inner/report.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&root, root.join("inner/loop")).unwrap();
    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let res = find_by_name(
        &registry(),
        &loc,
        "report",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res.hits.len(), 1);
}

#[tokio::test]
async fn noise_folders_match_by_name_but_are_not_searched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("app/node_modules/report-lib")).unwrap();
    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    std::fs::write(root.join(".git/report.pack"), "x").unwrap();
    std::fs::write(root.join("app/report.md"), "x").unwrap();
    let loc = Location::new(LocalFs::ID, root.to_string_lossy());

    let res = find_by_name(
        &registry(),
        &loc,
        "report",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let names: Vec<&str> = res.hits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["report.md"]);
    // The folder names themselves are still found.
    let res = find_by_name(
        &registry(),
        &loc,
        "node_mod",
        SearchOptions::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res.hits.len(), 1);

    // With skip_noise off, everything is searched.
    let all = SearchOptions {
        skip_noise: false,
        ..Default::default()
    };
    let res = find_by_name(&registry(), &loc, "report", all, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(res.hits.len(), 3, "{:?}", res.hits);
}

#[tokio::test]
async fn progress_reports_every_match_once_as_it_goes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for i in 0..100 {
        let dir = root.join(format!("d{i:03}"));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join(format!("match-{i}.txt")), "x").unwrap();
        std::fs::write(dir.join("sub/match-deep.txt"), "x").unwrap();
    }
    let loc = Location::new(LocalFs::ID, root.to_string_lossy());
    let mut reports = Vec::new();
    let res = dualpal_core::search::find_by_name_with(
        &registry(),
        &loc,
        "match",
        SearchOptions::default(),
        &CancellationToken::new(),
        |p| reports.push(p),
    )
    .await
    .unwrap();
    assert_eq!(res.hits.len(), 200);
    // Reported matches add up to the result, in the same order.
    let streamed: Vec<String> = reports
        .iter()
        .flat_map(|p| p.hits.iter().map(|e| e.path.clone()))
        .collect();
    let final_: Vec<String> = res.hits.iter().map(|e| e.path.clone()).collect();
    assert_eq!(streamed, final_);
    // Several reports arrive before the end, with a growing folder count.
    assert!(reports.len() > 3, "{} reports", reports.len());
    assert!(reports.windows(2).all(|w| w[0].folders <= w[1].folders));
    assert_eq!(reports.last().unwrap().folders, 1 + 100 + 100);
    // Shallow matches come before deep ones.
    let first_deep = res
        .hits
        .iter()
        .position(|e| e.name == "match-deep.txt")
        .unwrap();
    assert!(res.hits[..first_deep]
        .iter()
        .all(|e| e.name != "match-deep.txt"));
    assert_eq!(first_deep, 100);
}

#[tokio::test]
async fn cancelling_during_a_long_search_stops_it() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..200 {
        std::fs::create_dir_all(tmp.path().join(format!("d{i}/e/f"))).unwrap();
    }
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let res = dualpal_core::search::find_by_name_with(
        &registry(),
        &loc,
        "zzz",
        SearchOptions::default(),
        &cancel,
        move |p| {
            if p.folders > 50 {
                c.cancel();
            }
        },
    )
    .await;
    assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
}

#[tokio::test]
async fn time_budget_stops_the_walk_and_says_so() {
    let tmp = tree();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let opts = SearchOptions {
        max_millis: 0,
        ..SearchOptions::default()
    };
    let res = find_by_name(&registry(), &loc, "report", opts, &CancellationToken::new())
        .await
        .unwrap();
    assert!(res.truncated);
    assert!(res.hits.is_empty());
}

// ---- content search and filters (MemFs / FaultyFs stand in for a server) ----

mod content {
    use std::sync::Arc;
    use std::time::Duration;

    use dualpal_core::search::{find, KindFilter, SearchCriteria, SearchOptions, SearchProgress};
    use dualpal_core::vfs::FileSystem;
    use dualpal_core::{CancellationToken, Error, Location, Registry};
    use dualpal_testkit::{FaultOp, FaultyFs, MemFs};

    fn mem() -> (MemFs, Registry) {
        let fs = MemFs::with_default_id();
        let reg = Registry::new();
        reg.register(Arc::new(fs.clone()));
        (fs, reg)
    }

    fn root() -> Location {
        Location::new("mem", "/")
    }

    async fn run(reg: &Registry, c: SearchCriteria) -> dualpal_core::search::SearchResult {
        find(
            reg,
            &root(),
            &c,
            SearchOptions::default(),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap()
    }

    fn names(r: &dualpal_core::search::SearchResult) -> Vec<String> {
        r.hits.iter().map(|h| h.name.clone()).collect()
    }

    fn text(t: &str) -> SearchCriteria {
        SearchCriteria {
            content: t.into(),
            ..Default::default()
        }
    }

    // SEARCH-006 — text is found in a streamed file wherever the 64 KB chunks
    // fall, with the first matching line as the snippet.
    #[tokio::test]
    async fn finds_text_across_the_chunk_boundary_on_a_server() {
        let (fs, reg) = mem();
        let mut big = vec![b'.'; 64 * 1024 - 3];
        big.extend_from_slice(b"needle here\nsecond needle\n");
        fs.seed_file("/docs/big.log", &big);
        fs.seed_file("/docs/small.txt", "nothing\nNeedle at line two\n");
        fs.seed_file("/docs/none.txt", "no match");
        let res = run(&reg, text("needle")).await;
        assert_eq!(names(&res), vec!["big.log", "small.txt"]);
        let small = res.hits.iter().find(|h| h.name == "small.txt").unwrap();
        let s = small.snippet.as_ref().unwrap();
        assert_eq!((s.line, s.text.as_str()), (2, "Needle at line two"));
        assert_eq!(res.files_read, 3);
        // Case matters when asked.
        let res = run(
            &reg,
            SearchCriteria {
                ignore_case: false,
                ..text("Needle")
            },
        )
        .await;
        assert_eq!(names(&res), vec!["small.txt"]);
    }

    // SEARCH-007 — binary files are skipped and counted unless included.
    #[tokio::test]
    async fn binary_files_are_skipped_unless_included() {
        let (fs, reg) = mem();
        fs.seed_file("/a.bin", b"\x00\x01needle");
        fs.seed_file("/b.txt", "needle");
        let res = run(&reg, text("needle")).await;
        assert_eq!(names(&res), vec!["b.txt"]);
        assert_eq!(res.binary, 1);
        let res = run(
            &reg,
            SearchCriteria {
                include_binary: true,
                ..text("needle")
            },
        )
        .await;
        assert_eq!(names(&res), vec!["a.bin", "b.txt"]);
    }

    // SEARCH-008 — regular expressions for names and text; a bad one is an
    // `invalid` error.
    #[tokio::test]
    async fn regex_names_and_text() {
        let (fs, reg) = mem();
        fs.seed_file("/src/main.rs", "fn main() {}\n");
        fs.seed_file("/src/lib.rs", "pub fn helper_42() {}\n");
        fs.seed_file("/Cargo.toml", "[package]\n");
        fs.seed_file("/notes.md", "fn in prose 42\n");
        let res = run(
            &reg,
            SearchCriteria {
                name: r"\.(rs|toml)$".into(),
                name_regex: true,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(names(&res), vec!["Cargo.toml", "lib.rs", "main.rs"]);
        let res = run(
            &reg,
            SearchCriteria {
                name: "*.rs;*.md".into(),
                content: r"fn \w+_\d+\(".into(),
                content_regex: true,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(names(&res), vec!["lib.rs"]);
        let err = find(
            &reg,
            &root(),
            &SearchCriteria {
                content: "(".into(),
                content_regex: true,
                ..Default::default()
            },
            SearchOptions::default(),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    }

    // SEARCH-009 — size, date, kind and hidden filters, alone or with a name.
    #[tokio::test]
    async fn size_date_kind_and_hidden_filters() {
        let (fs, reg) = mem();
        fs.seed_file("/small.txt", vec![b'x'; 10]);
        fs.seed_file("/big.txt", vec![b'x'; 5000]);
        fs.seed_file("/old.txt", "x");
        fs.seed_file("/.hidden.txt", "x");
        fs.seed_file("/.secret/inside.txt", "x");
        fs.seed_dir("/txt-folder");
        fs.set_modified("/old.txt", 1_000).await.unwrap();
        let now = dualpal_core::now_ms();

        let res = run(
            &reg,
            SearchCriteria {
                min_size: Some(100),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(names(&res), vec!["big.txt"]);
        let res = run(
            &reg,
            SearchCriteria {
                name: "txt".into(),
                max_size: Some(100),
                include_hidden: false,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            names(&res),
            vec!["old.txt", "small.txt"],
            "folders and hidden left out"
        );
        let res = run(
            &reg,
            SearchCriteria {
                older_than_ms: Some(now - 86_400_000),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(names(&res), vec!["old.txt"]);
        let res = run(
            &reg,
            SearchCriteria {
                name: "txt".into(),
                newer_than_ms: Some(now - 86_400_000),
                kind: KindFilter::Folders,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(names(&res), vec!["txt-folder"]);
        let res = run(
            &reg,
            SearchCriteria {
                name: "*.txt".into(),
                kind: KindFilter::Files,
                ..Default::default()
            },
        )
        .await;
        assert_eq!(
            res.hits.len(),
            5,
            "hidden included by default: {:?}",
            names(&res)
        );
        // Kind alone asks for nothing.
        let res = run(
            &reg,
            SearchCriteria {
                kind: KindFilter::Files,
                ..Default::default()
            },
        )
        .await;
        assert!(res.hits.is_empty());
    }

    // SEARCH-010 — on a server, files over the limit are not downloaded.
    #[tokio::test]
    async fn large_remote_files_are_not_read() {
        let fs = FaultyFs::new(MemFs::with_default_id());
        fs.inner().seed_file("/big.txt", vec![b'n'; 2000]);
        fs.inner().seed_file("/ok.txt", "needle");
        let fs = Arc::new(fs);
        let reg = Registry::new();
        reg.register(fs.clone());
        let opts = SearchOptions {
            remote_content_limit: 1000,
            ..Default::default()
        };
        let res = find(
            &reg,
            &root(),
            &text("n"),
            opts,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(names(&res), vec!["ok.txt"]);
        assert_eq!(res.too_large, 1);
        assert_eq!(fs.read_count(), 1, "only the small file was opened");
    }

    // SEARCH-011 — a file that cannot be read is counted and the search goes on.
    #[tokio::test]
    async fn unreadable_files_are_counted() {
        let fs = FaultyFs::new(MemFs::with_default_id());
        fs.inner().seed_file("/a.txt", "needle");
        fs.inner().seed_file("/b.txt", "needle");
        fs.fail_next(FaultOp::OpenRead, Error::Invalid("boom".into()));
        let reg = Registry::new();
        reg.register(Arc::new(fs));
        let res = run(&reg, text("needle")).await;
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.files_skipped, 1);
    }

    // SEARCH-012 — cancelling while files are being read stops at once, and
    // progress counts the files read.
    #[tokio::test]
    async fn cancelling_a_slow_content_search() {
        let fs = FaultyFs::new(MemFs::with_default_id());
        for i in 0..200 {
            fs.inner().seed_file(&format!("/f{i:03}.txt"), "needle");
        }
        fs.latency(FaultOp::OpenRead, Duration::from_millis(20));
        let reg = Registry::new();
        reg.register(Arc::new(fs));
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        let mut seen: Vec<SearchProgress> = Vec::new();
        let started = std::time::Instant::now();
        let res = find(
            &reg,
            &root(),
            &text("needle"),
            SearchOptions::default(),
            &cancel,
            |p| {
                if p.files >= 5 {
                    c.cancel();
                }
                seen.push(p);
            },
        )
        .await;
        assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(seen.iter().any(|p| p.files >= 5 && !p.hits.is_empty()));
    }
}

// SEARCH-013 — content search on the local disk streams a file larger than
// one chunk and reads no size limit.
#[tokio::test]
async fn local_content_search_streams_big_files() {
    use dualpal_core::search::{find, SearchCriteria};
    let tmp = tempfile::tempdir().unwrap();
    let mut big = vec![b'a'; 3 * 64 * 1024 + 5];
    big.extend_from_slice(b"\nfind ME please\n");
    std::fs::write(tmp.path().join("big.txt"), &big).unwrap();
    std::fs::write(tmp.path().join("other.txt"), "nothing").unwrap();
    let loc = Location::new(LocalFs::ID, tmp.path().to_string_lossy());
    let opts = SearchOptions {
        remote_content_limit: 10,
        ..Default::default()
    };
    let c = SearchCriteria {
        name: "*.txt".into(),
        content: "find me".into(),
        ..Default::default()
    };
    let res = find(
        &registry(),
        &loc,
        &c,
        opts,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(res.hits.len(), 1);
    let s = res.hits[0].snippet.as_ref().unwrap();
    assert_eq!((s.line, s.text.as_str()), (2, "find ME please"));
    assert_eq!(res.too_large, 0);
}

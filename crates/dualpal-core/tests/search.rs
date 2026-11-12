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

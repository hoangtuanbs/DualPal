//! Diff tool cases (Phase 1, §8.11).

use dualpal_core::difftools::{command_line, find_program};

/// DIFF-003 — quotes and spaces survive the template and the paths.
#[test]
fn quotes_and_spaces() {
    let argv = command_line(
        r#""C:\Program Files\X\x.exe" "{left}" "{right}""#,
        "a b.txt",
        r#"c "d".txt"#,
    )
    .unwrap();
    assert_eq!(
        argv,
        vec![r"C:\Program Files\X\x.exe", "a b.txt", r#"c "d".txt"#]
    );
}

/// DIFF-004 — a template without both tokens still appends the paths.
///
/// The current implementation appends rather than erroring, which is the
/// documented behaviour for tools like `meld`.
#[test]
fn template_without_tokens_appends_paths() {
    assert_eq!(
        command_line("meld", "L", "R").unwrap(),
        vec!["meld", "L", "R"]
    );
}

/// DIFF-005 — a tool on PATH is detected.
#[test]
fn detect_finds_a_tool_on_path() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let tool = dir.path().join("meld");
    #[cfg(windows)]
    let tool = dir.path().join("meld.cmd");
    std::fs::write(&tool, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // PATH lookup in `find_program` honours the current process PATH, so put
    // the temp dir in front.
    let old = std::env::var_os("PATH");
    let mut paths = std::env::split_paths(&old.clone().unwrap_or_default()).collect::<Vec<_>>();
    paths.insert(0, dir.path().to_path_buf());
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

    assert!(
        find_program("meld").is_some(),
        "meld should be found on PATH"
    );
    if let Some(old) = old {
        std::env::set_var("PATH", old);
    }
}

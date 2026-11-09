//! Recording "opened with" actions instead of launching them.
//!
//! End-to-end tests cannot watch a desktop app spawn a browser or a diff tool.
//! When `DUALPAL_DIFF_TOOL_LOG` or `DUALPAL_OPENER_LOG` is set, the action is
//! appended to that file instead. Nothing is recorded when the variable is
//! not set, so the shipped app behaves exactly as before.

use std::io::Write;

/// Append `line` to the file named by `var`, returning whether it was
/// recorded. An unset variable returns `false`; a write error is reported on
/// stderr and also returns `false` so the caller can fall back to launching.
pub fn record(var: &str, line: &str) -> bool {
    let Some(path) = std::env::var_os(var) else {
        return false;
    };
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| writeln!(f, "{line}"));
    match result {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "{var} is set to {} but could not be written: {e}",
                std::path::Path::new(&path).display()
            );
            false
        }
    }
}

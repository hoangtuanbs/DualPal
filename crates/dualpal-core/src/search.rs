//! Find files by name below a folder, for the Find dialog.
//!
//! [`find_by_name`] walks the folder tree breadth-first through the
//! [`FileSystem`] trait, so shallow matches come first and the same search
//! works on the local disk, a server or a bucket.

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{Entry, EntryKind, Location, Registry};

/// How far and how many results a search may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    /// Stop after this many matches. The result is marked `truncated`.
    pub max_results: usize,
    /// Do not descend deeper than this many folder levels below the start.
    pub max_depth: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_results: 1000,
            max_depth: 32,
        }
    }
}

/// What a search found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Matching entries, shallowest first.
    pub hits: Vec<Entry>,
    /// True when `max_results` was reached, so more matches may exist.
    pub truncated: bool,
    /// Subfolders that could not be read and were left out.
    pub skipped: u64,
}

/// Whether `name` matches `query`: a case-insensitive substring test.
/// An empty or all-space query matches nothing.
pub fn name_matches(name: &str, query: &str) -> bool {
    let q = query.trim();
    !q.is_empty() && name.to_lowercase().contains(&q.to_lowercase())
}

/// Search the folder at `location` and everything below it for names that
/// match `query`. Symbolic links to folders are not followed. The walk stops
/// with [`Error::Cancelled`] once `cancel` is set.
pub async fn find_by_name(
    registry: &Registry,
    location: &Location,
    query: &str,
    options: SearchOptions,
    cancel: &CancellationToken,
) -> Result<SearchResult> {
    let fs = registry.get(&location.fs)?;
    let mut result = SearchResult::default();
    if query.trim().is_empty() {
        return Ok(result);
    }
    if options.max_results == 0 {
        result.truncated = true;
        return Ok(result);
    }
    // Breadth-first: a queue of (folder path, depth below the start).
    let mut queue = std::collections::VecDeque::from([(location.path.clone(), 0usize)]);
    let mut is_root = true;
    while let Some((dir, depth)) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let listing = tokio::select! {
            r = fs.list(&dir) => r,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let entries = match listing {
            Ok(entries) => entries,
            Err(e) if is_root => return Err(e),
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        is_root = false;
        for e in entries {
            if name_matches(&e.name, query) {
                result.hits.push(e.clone());
                // Stop at the cap right away rather than looking for one more match.
                if result.hits.len() >= options.max_results {
                    result.truncated = true;
                    return Ok(result);
                }
            }
            if e.kind == EntryKind::Dir && !e.symlink && depth < options.max_depth {
                queue.push_back((e.path, depth + 1));
            }
        }
    }
    Ok(result)
}

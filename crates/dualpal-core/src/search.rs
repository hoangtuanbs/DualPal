//! Find files by name below a folder, for the Find dialog.
//!
//! [`find_by_name`] walks the folder tree breadth-first through the
//! [`FileSystem`] trait, so shallow matches come first and the same search
//! works on the local disk, a server or a bucket. Several folders of a level
//! are listed at once, and [`find_by_name_with`] reports matches as they are
//! found, so searching a large tree shows results right away.

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{Entry, EntryKind, Location, Registry};

/// How far and how many results a search may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchOptions {
    /// Stop after this many matches. The result is marked `truncated`.
    pub max_results: usize,
    /// Do not descend deeper than this many folder levels below the start.
    pub max_depth: usize,
    /// Do not look inside folders that only hold tool data, such as `.git`
    /// and `node_modules` ([`NOISE_FOLDERS`]). Their own names still match.
    pub skip_noise: bool,
    /// Stop after this many milliseconds, returning what was found so far
    /// with `truncated` set. Keeps a search of a huge tree from running on.
    pub max_millis: u64,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_results: 1000,
            max_depth: 32,
            skip_noise: true,
            max_millis: 10_000,
        }
    }
}

/// Folders full of files nobody looks for by name. Walking them is what
/// makes a search of a home folder or a project take minutes.
pub const NOISE_FOLDERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "__pycache__",
    ".venv",
    "$RECYCLE.BIN",
    "System Volume Information",
];

/// Folders listed at the same time. Remote storage gains most, since each
/// listing waits on the network.
const PARALLEL_LISTINGS: usize = 8;

/// Report progress after this many folders, even within one level.
const REPORT_EVERY: u64 = 32;

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

/// Progress of a running search: the matches found since the last report,
/// and how many folders have been read so far.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub hits: Vec<Entry>,
    pub folders: u64,
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
    find_by_name_with(registry, location, query, options, cancel, |_| {}).await
}

/// Collects matches and the folders to read next while a search runs.
struct Walk<'a> {
    query: String,
    options: &'a SearchOptions,
    result: SearchResult,
    /// Matches since the last progress report.
    fresh: Vec<Entry>,
    /// Folders of the next level.
    next: Vec<String>,
}

impl Walk<'_> {
    /// Take in one folder listing at `depth`. Returns true once the result
    /// cap is reached.
    fn take(&mut self, entries: Vec<Entry>, depth: usize) -> bool {
        for e in entries {
            let descend = e.kind == EntryKind::Dir
                && !e.symlink
                && depth < self.options.max_depth
                && !(self.options.skip_noise && NOISE_FOLDERS.contains(&e.name.as_str()));
            if e.name.to_lowercase().contains(&self.query) {
                self.fresh.push(e.clone());
                self.result.hits.push(e.clone());
                // Stop at the cap right away rather than looking for one more match.
                if self.result.hits.len() >= self.options.max_results {
                    self.result.truncated = true;
                    return true;
                }
            }
            if descend {
                self.next.push(e.path);
            }
        }
        false
    }

    fn report(&mut self, folders: u64, progress: &mut impl FnMut(SearchProgress)) {
        progress(SearchProgress {
            hits: std::mem::take(&mut self.fresh),
            folders,
        });
    }
}

/// [`find_by_name`], calling `progress` with the new matches and the number
/// of folders read: after the start folder, every [`REPORT_EVERY`] folders,
/// and at the end of each level.
pub async fn find_by_name_with(
    registry: &Registry,
    location: &Location,
    query: &str,
    options: SearchOptions,
    cancel: &CancellationToken,
    mut progress: impl FnMut(SearchProgress),
) -> Result<SearchResult> {
    let fs = registry.get(&location.fs)?;
    if query.trim().is_empty() {
        return Ok(SearchResult::default());
    }
    if options.max_results == 0 {
        return Ok(SearchResult {
            truncated: true,
            ..Default::default()
        });
    }
    let mut walk = Walk {
        query: query.trim().to_lowercase(),
        options: &options,
        result: SearchResult::default(),
        fresh: Vec::new(),
        next: Vec::new(),
    };

    // Past this point the search returns what it has, marked truncated.
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(options.max_millis);
    if options.max_millis == 0 {
        walk.result.truncated = true;
        return Ok(walk.result);
    }

    // The start folder must be readable; folders below it are skipped if not.
    let root = tokio::select! {
        r = fs.list(&location.path) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            walk.result.truncated = true;
            return Ok(walk.result);
        }
    };
    let mut folders = 1u64;
    let done = walk.take(root, 0);
    walk.report(folders, &mut progress);
    if done {
        return Ok(walk.result);
    }

    let mut depth = 0usize;
    while !walk.next.is_empty() {
        depth += 1;
        let dirs = std::mem::take(&mut walk.next);
        // List the level several folders at a time, keeping their order so
        // results stay in a stable, shallow-first order.
        let mut listings = stream::iter(dirs)
            .map(|dir| {
                let fs = fs.clone();
                async move { fs.list(&dir).await }
            })
            .buffered(PARALLEL_LISTINGS);
        loop {
            let item = tokio::select! {
                item = listings.next() => item,
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                _ = tokio::time::sleep_until(deadline) => {
                    walk.result.truncated = true;
                    walk.report(folders, &mut progress);
                    return Ok(walk.result);
                }
            };
            let Some(listing) = item else { break };
            folders += 1;
            match listing {
                Ok(entries) => {
                    if walk.take(entries, depth) {
                        walk.report(folders, &mut progress);
                        return Ok(walk.result);
                    }
                }
                Err(_) => walk.result.skipped += 1,
            }
            if folders.is_multiple_of(REPORT_EVERY) {
                walk.report(folders, &mut progress);
            }
        }
        walk.report(folders, &mut progress);
    }
    Ok(walk.result)
}

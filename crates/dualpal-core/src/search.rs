//! Find files below a folder, for the Find dialog.
//!
//! [`find`] walks the folder tree breadth-first through the [`FileSystem`]
//! trait, so shallow matches come first and the same search works on the
//! local disk, a server or a bucket. Several folders of a level are listed at
//! once, and matches are reported as they are found, so searching a large
//! tree shows results right away.
//!
//! A search can match names (substrings, `*.rs;*.toml` masks or a regular
//! expression), filter by kind, size, date and hidden flag, and look for text
//! inside files. File contents are streamed in chunks and matched line by
//! line, so big files are never loaded whole; files that look binary (a NUL
//! byte near the start) are skipped unless asked for. On servers, files over
//! [`SearchOptions::remote_content_limit`] are not read.
//!
//! [`FileSystem`]: crate::vfs::FileSystem

use std::ops::Deref;
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use regex::bytes::Regex as BytesRegex;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{Entry, EntryKind, FileSystem, LocalFs, Location, Registry};

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
    /// On anything but the local disk, files larger than this are not
    /// searched for text (each one would be downloaded). They are counted in
    /// [`SearchResult::too_large`].
    pub remote_content_limit: u64,
    /// Bytes read from a file at a time when searching its text.
    pub chunk_size: usize,
}

/// Default for [`SearchOptions::remote_content_limit`]: 10 MB.
pub const REMOTE_CONTENT_LIMIT: u64 = 10_000_000;

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            max_results: 1000,
            max_depth: 32,
            skip_noise: true,
            max_millis: 10_000,
            remote_content_limit: REMOTE_CONTENT_LIMIT,
            chunk_size: 64 * 1024,
        }
    }
}

/// Which kinds of entries a search returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KindFilter {
    #[default]
    Any,
    Files,
    Folders,
}

/// What to look for. Every part that is set must match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchCriteria {
    /// Part of the name, or masks such as `*.rs;*.toml` (a mask with `*` or
    /// `?` must match the whole name; one without matches any part of it).
    /// With `name_regex`, a regular expression found anywhere in the name.
    /// Empty matches every name.
    pub name: String,
    pub name_regex: bool,
    /// Text to find inside files; empty searches names only. Matched line by
    /// line. With `content_regex`, a regular expression.
    pub content: String,
    pub content_regex: bool,
    /// Letter case does not matter, in names and in text.
    pub ignore_case: bool,
    /// Also search the text of files that look binary.
    pub include_binary: bool,
    /// Size limits in bytes. Setting either one leaves folders out.
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Modified at or after / before these times (ms since the epoch).
    pub newer_than_ms: Option<i64>,
    pub older_than_ms: Option<i64>,
    pub kind: KindFilter,
    /// Hidden items are found, and hidden folders searched.
    pub include_hidden: bool,
}

impl Default for SearchCriteria {
    fn default() -> Self {
        Self {
            name: String::new(),
            name_regex: false,
            content: String::new(),
            content_regex: false,
            ignore_case: true,
            include_binary: false,
            min_size: None,
            max_size: None,
            newer_than_ms: None,
            older_than_ms: None,
            kind: KindFilter::Any,
            include_hidden: true,
        }
    }
}

impl SearchCriteria {
    /// Criteria that match names containing `query`, as the name-only search did.
    pub fn named(query: &str) -> Self {
        Self {
            name: query.to_string(),
            ..Self::default()
        }
    }

    /// Whether anything is asked for at all. Kind and hidden alone are not
    /// enough: they would list the whole tree.
    pub fn is_empty(&self) -> bool {
        self.name.trim().is_empty()
            && self.content.is_empty()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.newer_than_ms.is_none()
            && self.older_than_ms.is_none()
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

/// Files read at the same time on the local disk.
const PARALLEL_LOCAL_READS: usize = 4;

/// Report progress after this many folders, even within one level.
const REPORT_EVERY: u64 = 32;

/// Report progress after this many files read, even within one folder.
const REPORT_EVERY_FILES: u64 = 64;

/// A file with a NUL byte in its first this many bytes is binary.
pub const BINARY_PROBE: usize = 8 * 1024;

/// A line longer than this is searched in pieces, so a file without line
/// breaks is never held in memory whole.
const MAX_LINE: usize = 1024 * 1024;

/// Bytes kept from the end of one piece of a very long line for the next,
/// so that a match across the cut is still found.
const LONG_LINE_OVERLAP: usize = 4 * 1024;

/// Characters of a matching line shown as the snippet.
const SNIPPET_CHARS: usize = 160;

/// Where text was found in a file: the first matching line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// Line number, from 1.
    pub line: u64,
    /// The line (or the part of a long line around the match), trimmed.
    pub text: String,
}

/// One match: the entry, plus where its text matched for a content search.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    #[serde(flatten)]
    pub entry: Entry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<Snippet>,
}

impl Deref for SearchHit {
    type Target = Entry;
    fn deref(&self) -> &Entry {
        &self.entry
    }
}

/// What a search found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Matching entries, shallowest first.
    pub hits: Vec<SearchHit>,
    /// True when `max_results` was reached, so more matches may exist.
    pub truncated: bool,
    /// Subfolders that could not be read and were left out.
    pub skipped: u64,
    /// Files whose text was searched.
    pub files_read: u64,
    /// Files that could not be opened or read for a text search.
    pub files_skipped: u64,
    /// Files on a server too large to search for text
    /// ([`SearchOptions::remote_content_limit`]).
    pub too_large: u64,
    /// Files left out of a text search because they look binary.
    pub binary: u64,
}

/// Progress of a running search: the matches found since the last report,
/// how many folders have been listed and how many files read so far.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgress {
    pub hits: Vec<SearchHit>,
    pub folders: u64,
    pub files: u64,
}

/// Whether `name` matches `query`: a case-insensitive substring test.
/// An empty or all-space query matches nothing.
pub fn name_matches(name: &str, query: &str) -> bool {
    let q = query.trim();
    !q.is_empty() && name.to_lowercase().contains(&q.to_lowercase())
}

/// The name part of [`SearchCriteria`], compiled.
#[derive(Clone, Debug)]
pub struct NameMatcher(Option<Regex>);

impl NameMatcher {
    /// Compile `pattern`. See [`SearchCriteria::name`] for the syntax.
    pub fn new(pattern: &str, regex: bool, ignore_case: bool) -> Result<Self> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Ok(Self(None));
        }
        let source = if regex {
            pattern.to_string()
        } else {
            let masks: Vec<String> = pattern
                .split(';')
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(mask_regex)
                .collect();
            if masks.is_empty() {
                return Ok(Self(None));
            }
            masks.join("|")
        };
        RegexBuilder::new(&source)
            .case_insensitive(ignore_case)
            .build()
            .map(|r| Self(Some(r)))
            .map_err(|e| Error::Invalid(format!("name pattern: {}", first_line(&e.to_string()))))
    }

    pub fn matches(&self, name: &str) -> bool {
        self.0.as_ref().is_none_or(|r| r.is_match(name))
    }
}

/// One mask as a regular expression: `*` and `?` wildcards over the whole
/// name, or a plain substring when it has neither.
fn mask_regex(mask: &str) -> String {
    if !mask.contains(['*', '?']) {
        return format!("(?:{})", regex::escape(mask));
    }
    let mut out = String::from("(?:^");
    for c in mask.chars() {
        match c {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out.push_str("$)");
    out
}

fn first_line(s: &str) -> &str {
    // regex errors draw the pattern with a caret on several lines; the last
    // line says what is wrong.
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(s)
        .trim()
}

/// The text part of [`SearchCriteria`], compiled.
#[derive(Clone, Debug)]
pub struct ContentMatcher(BytesRegex);

impl ContentMatcher {
    pub fn new(text: &str, regex: bool, ignore_case: bool) -> Result<Self> {
        let source = if regex {
            text.to_string()
        } else {
            regex::escape(text)
        };
        regex::bytes::RegexBuilder::new(&source)
            .case_insensitive(ignore_case)
            .build()
            .map(Self)
            .map_err(|e| Error::Invalid(format!("text pattern: {}", first_line(&e.to_string()))))
    }
}

/// What reading one file for text found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentScan {
    Found(Snippet),
    NotFound,
    /// A NUL byte in the first [`BINARY_PROBE`] bytes, with binary files left out.
    Binary,
}

/// Read `reader` `chunk` bytes at a time and look for `matcher` line by
/// line. Lines split across chunks are joined before matching, so a match is
/// found wherever the chunks fall. Stops at the first matching line.
pub async fn scan_content<R: AsyncRead + Unpin>(
    reader: &mut R,
    matcher: &ContentMatcher,
    include_binary: bool,
    chunk: usize,
) -> std::io::Result<ContentScan> {
    let chunk = chunk.max(1);
    let re = &matcher.0;
    // Bytes not yet matched: the start of a line whose end has not been read.
    let mut buf: Vec<u8> = Vec::with_capacity(chunk * 2);
    let mut probed = 0usize;
    let mut line = 1u64;
    loop {
        let start = buf.len();
        buf.resize(start + chunk, 0);
        let n = reader.read(&mut buf[start..]).await?;
        buf.truncate(start + n);
        let eof = n == 0;
        if probed < BINARY_PROBE {
            let end = (start + n).min(start + BINARY_PROBE - probed);
            if !include_binary && buf[start..end].contains(&0) {
                return Ok(ContentScan::Binary);
            }
            probed += n;
            // Match nothing until the start of the file is known to be text.
            if !include_binary && probed < BINARY_PROBE && !eof {
                continue;
            }
        }
        let mut used = 0;
        while let Some(pos) = buf[used..].iter().position(|&b| b == b'\n') {
            let text = &buf[used..used + pos];
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            if let Some(m) = re.find(text) {
                return Ok(ContentScan::Found(snippet(text, m.start(), line)));
            }
            line += 1;
            used += pos + 1;
        }
        buf.drain(..used);
        if eof {
            if let Some(m) = re.find(&buf) {
                return Ok(ContentScan::Found(snippet(&buf, m.start(), line)));
            }
            return Ok(ContentScan::NotFound);
        }
        if buf.len() > MAX_LINE {
            // One very long line: search what there is and keep its end for
            // a match that runs into the next piece.
            if let Some(m) = re.find(&buf) {
                return Ok(ContentScan::Found(snippet(&buf, m.start(), line)));
            }
            buf.drain(..buf.len() - LONG_LINE_OVERLAP);
        }
    }
}

/// The matching line as text to show: around the match when the line is long.
fn snippet(line: &[u8], at: usize, number: u64) -> Snippet {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let text = String::from_utf8_lossy(line);
    // Byte offset in `line` → char offset in `text` (close enough with
    // replacement characters; only used to centre the window).
    let at_char = String::from_utf8_lossy(&line[..at.min(line.len())])
        .chars()
        .count();
    let chars: Vec<char> = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let lead = SNIPPET_CHARS / 4;
    let (mut from, mut to) = (0, chars.len());
    if chars.len() > SNIPPET_CHARS {
        from = at_char.saturating_sub(lead);
        to = (from + SNIPPET_CHARS).min(chars.len());
        from = to.saturating_sub(SNIPPET_CHARS);
    }
    let mut out: String = chars[from..to]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    if from > 0 {
        out.insert(0, '…');
    }
    if to < chars.len() {
        out.push('…');
    }
    Snippet {
        line: number,
        text: out,
    }
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

/// [`find_by_name`], calling `progress` as [`find`] does.
pub async fn find_by_name_with(
    registry: &Registry,
    location: &Location,
    query: &str,
    options: SearchOptions,
    cancel: &CancellationToken,
    progress: impl FnMut(SearchProgress),
) -> Result<SearchResult> {
    find(
        registry,
        location,
        &SearchCriteria::named(query),
        options,
        cancel,
        progress,
    )
    .await
}

/// The compiled criteria.
struct Matcher {
    name: NameMatcher,
    content: Option<ContentMatcher>,
    criteria: SearchCriteria,
}

impl Matcher {
    /// Whether `e` passes everything but the text test.
    fn passes(&self, e: &Entry) -> bool {
        let c = &self.criteria;
        let kind_ok = match c.kind {
            KindFilter::Any => true,
            KindFilter::Files => e.kind == EntryKind::File,
            KindFilter::Folders => e.kind == EntryKind::Dir,
        };
        let sized = c.min_size.is_some() || c.max_size.is_some();
        let size_ok = !sized
            || (e.kind == EntryKind::File
                && c.min_size.is_none_or(|m| e.size >= m)
                && c.max_size.is_none_or(|m| e.size <= m));
        let dated = c.newer_than_ms.is_some() || c.older_than_ms.is_some();
        let date_ok = !dated
            || e.modified_ms.is_some_and(|t| {
                c.newer_than_ms.is_none_or(|n| t >= n) && c.older_than_ms.is_none_or(|o| t < o)
            });
        // Text is only ever found in files.
        let content_ok = self.content.is_none() || e.kind == EntryKind::File;
        kind_ok && size_ok && date_ok && content_ok && self.name.matches(&e.name)
    }
}

/// What checking one candidate file for text came to.
enum Checked {
    Found(Snippet),
    NotFound,
    Binary,
    Failed,
}

async fn check_file(
    fs: &dyn FileSystem,
    path: &str,
    matcher: &ContentMatcher,
    include_binary: bool,
    chunk: usize,
) -> Checked {
    let Ok(mut reader) = fs.open_read(path).await else {
        return Checked::Failed;
    };
    match scan_content(&mut reader, matcher, include_binary, chunk).await {
        Ok(ContentScan::Found(s)) => Checked::Found(s),
        Ok(ContentScan::NotFound) => Checked::NotFound,
        Ok(ContentScan::Binary) => Checked::Binary,
        Err(_) => Checked::Failed,
    }
}

/// Collects matches and the folders to read next while a search runs.
struct Walk<'a> {
    matcher: Matcher,
    options: &'a SearchOptions,
    /// File reads at the same time.
    readers: usize,
    /// Files over this size are not read (`u64::MAX` on the local disk).
    content_limit: u64,
    result: SearchResult,
    /// Matches since the last progress report.
    fresh: Vec<SearchHit>,
    /// Folders of the next level.
    next: Vec<String>,
    folders: u64,
}

/// Why a walk ends early.
enum Stop {
    /// The result cap or the time budget was reached: return what there is.
    Done,
    Cancelled,
}

impl Walk<'_> {
    fn hit(&mut self, entry: Entry, snippet: Option<Snippet>) -> bool {
        let hit = SearchHit { entry, snippet };
        self.fresh.push(hit.clone());
        self.result.hits.push(hit);
        // Stop at the cap right away rather than looking for one more match.
        if self.result.hits.len() >= self.options.max_results {
            self.result.truncated = true;
            return true;
        }
        false
    }

    /// Take in one folder listing at `depth`: record name matches, read the
    /// candidate files for text, and queue the subfolders.
    async fn take(
        &mut self,
        fs: &Arc<dyn FileSystem>,
        entries: Vec<Entry>,
        depth: usize,
        cancel: &CancellationToken,
        deadline: tokio::time::Instant,
        progress: &mut impl FnMut(SearchProgress),
    ) -> std::result::Result<(), Stop> {
        let include_hidden = self.matcher.criteria.include_hidden;
        let mut candidates = Vec::new();
        for e in entries {
            if e.hidden && !include_hidden {
                continue;
            }
            let descend = e.kind == EntryKind::Dir
                && !e.symlink
                && depth < self.options.max_depth
                && !(self.options.skip_noise && NOISE_FOLDERS.contains(&e.name.as_str()));
            if descend {
                self.next.push(e.path.clone());
            }
            if !self.matcher.passes(&e) {
                continue;
            }
            if self.matcher.content.is_none() {
                if self.hit(e, None) {
                    return Err(Stop::Done);
                }
            } else if e.size > self.content_limit {
                self.result.too_large += 1;
            } else {
                candidates.push(e);
            }
        }
        let Some(content) = self.matcher.content.clone() else {
            return Ok(());
        };
        if candidates.is_empty() {
            return Ok(());
        }
        let include_binary = self.matcher.criteria.include_binary;
        let chunk = self.options.chunk_size;
        // Read several files at once, keeping their order.
        let mut checks = stream::iter(candidates)
            .map(|e| {
                let fs = fs.clone();
                let content = content.clone();
                async move {
                    let out =
                        check_file(fs.as_ref(), &e.path, &content, include_binary, chunk).await;
                    (e, out)
                }
            })
            .buffered(self.readers);
        loop {
            let item = tokio::select! {
                item = checks.next() => item,
                _ = cancel.cancelled() => return Err(Stop::Cancelled),
                _ = tokio::time::sleep_until(deadline) => {
                    self.result.truncated = true;
                    return Err(Stop::Done);
                }
            };
            let Some((entry, checked)) = item else { break };
            self.result.files_read += 1;
            let mut report = self.result.files_read.is_multiple_of(REPORT_EVERY_FILES);
            match checked {
                Checked::Found(s) => {
                    if self.hit(entry, Some(s)) {
                        return Err(Stop::Done);
                    }
                    report = true;
                }
                Checked::NotFound => {}
                Checked::Binary => self.result.binary += 1,
                Checked::Failed => self.result.files_skipped += 1,
            }
            if report {
                self.report(progress);
            }
        }
        Ok(())
    }

    fn report(&mut self, progress: &mut impl FnMut(SearchProgress)) {
        progress(SearchProgress {
            hits: std::mem::take(&mut self.fresh),
            folders: self.folders,
            files: self.result.files_read,
        });
    }
}

/// Search the folder at `location` and everything below it for entries that
/// meet `criteria`. Symbolic links to folders are not followed. `progress` is
/// called with the new matches and the folders and files read so far: after
/// the start folder, every [`REPORT_EVERY`] folders or
/// [`REPORT_EVERY_FILES`] files, on each text match, and at the end of each
/// level. The walk stops with [`Error::Cancelled`] once `cancel` is set.
pub async fn find(
    registry: &Registry,
    location: &Location,
    criteria: &SearchCriteria,
    options: SearchOptions,
    cancel: &CancellationToken,
    mut progress: impl FnMut(SearchProgress),
) -> Result<SearchResult> {
    let fs = registry.get(&location.fs)?;
    if criteria.is_empty() {
        return Ok(SearchResult::default());
    }
    let matcher = Matcher {
        name: NameMatcher::new(&criteria.name, criteria.name_regex, criteria.ignore_case)?,
        content: if criteria.content.is_empty() {
            None
        } else {
            Some(ContentMatcher::new(
                &criteria.content,
                criteria.content_regex,
                criteria.ignore_case,
            )?)
        },
        criteria: criteria.clone(),
    };
    if options.max_results == 0 {
        return Ok(SearchResult {
            truncated: true,
            ..Default::default()
        });
    }
    let local = fs.id() == LocalFs::ID;
    let mut walk = Walk {
        matcher,
        options: &options,
        readers: if local {
            PARALLEL_LOCAL_READS
        } else {
            fs.parallel_transfers().clamp(1, PARALLEL_LOCAL_READS)
        },
        content_limit: if local {
            u64::MAX
        } else {
            options.remote_content_limit
        },
        result: SearchResult::default(),
        fresh: Vec::new(),
        next: Vec::new(),
        folders: 0,
    };

    // Past this point the search returns what it has, marked truncated.
    let now = tokio::time::Instant::now();
    let budget = std::time::Duration::from_millis(options.max_millis);
    // A budget too large to add is as good as none (about thirty years).
    let deadline = now
        .checked_add(budget)
        .unwrap_or(now + std::time::Duration::from_secs(1 << 30));
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
    walk.folders = 1;
    let ended = walk
        .take(&fs, root, 0, cancel, deadline, &mut progress)
        .await;
    walk.report(&mut progress);
    match ended {
        Ok(()) => {}
        Err(Stop::Done) => return Ok(walk.result),
        Err(Stop::Cancelled) => return Err(Error::Cancelled),
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
                    walk.report(&mut progress);
                    return Ok(walk.result);
                }
            };
            let Some(listing) = item else { break };
            walk.folders += 1;
            match listing {
                Ok(entries) => {
                    match walk
                        .take(&fs, entries, depth, cancel, deadline, &mut progress)
                        .await
                    {
                        Ok(()) => {}
                        Err(Stop::Done) => {
                            walk.report(&mut progress);
                            return Ok(walk.result);
                        }
                        Err(Stop::Cancelled) => return Err(Error::Cancelled),
                    }
                }
                Err(_) => walk.result.skipped += 1,
            }
            if walk.folders.is_multiple_of(REPORT_EVERY) {
                walk.report(&mut progress);
            }
        }
        walk.report(&mut progress);
    }
    Ok(walk.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader that hands out at most `step` bytes per read.
    struct Trickle {
        data: Vec<u8>,
        at: usize,
        step: usize,
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let n = self
                .step
                .min(buf.remaining())
                .min(self.data.len() - self.at);
            let at = self.at;
            buf.put_slice(&self.data[at..at + n]);
            self.at += n;
            std::task::Poll::Ready(Ok(()))
        }
    }

    async fn scan(data: &[u8], text: &str, regex: bool, chunk: usize, step: usize) -> ContentScan {
        let m = ContentMatcher::new(text, regex, true).unwrap();
        let mut r = Trickle {
            data: data.to_vec(),
            at: 0,
            step,
        };
        scan_content(&mut r, &m, false, chunk).await.unwrap()
    }

    // SEARCH-001 — a match is found wherever the chunks are cut, with the
    // right line number.
    #[tokio::test]
    async fn matches_across_every_chunk_boundary() {
        let mut data = Vec::new();
        for i in 0..50 {
            data.extend_from_slice(format!("line {i} nothing here\n").as_bytes());
        }
        data.extend_from_slice(b"the Needle in the haystack\nafter\n");
        for chunk in 1..40 {
            match scan(&data, "needle", false, chunk, chunk).await {
                ContentScan::Found(s) => {
                    assert_eq!(s.line, 51, "chunk {chunk}");
                    assert_eq!(s.text, "the Needle in the haystack");
                }
                other => panic!("chunk {chunk}: {other:?}"),
            }
        }
    }

    // SEARCH-002 — a regex never matches across lines, and a last line
    // without a line break is still searched.
    #[tokio::test]
    async fn regex_is_per_line_and_last_line_counts() {
        let data = b"alpha\nbeta\r\ngamma 42";
        assert!(matches!(
            scan(data, "alpha.beta", true, 3, 3).await,
            ContentScan::NotFound
        ));
        match scan(data, r"gam+a \d+$", true, 4, 4).await {
            ContentScan::Found(s) => assert_eq!((s.line, s.text.as_str()), (3, "gamma 42")),
            other => panic!("{other:?}"),
        }
        match scan(data, "^beta$", true, 64, 64).await {
            ContentScan::Found(s) => assert_eq!(s.line, 2, "the \\r is not part of the line"),
            other => panic!("{other:?}"),
        }
    }

    // SEARCH-003 — a NUL byte near the start makes a file binary, even when
    // it arrives after a matching line in a later read.
    #[tokio::test]
    async fn nul_in_the_first_kilobytes_is_binary() {
        let mut data = b"needle\n".to_vec();
        data.extend(std::iter::repeat_n(b'x', 3000));
        data.push(0);
        assert_eq!(
            scan(&data, "needle", false, 16, 16).await,
            ContentScan::Binary
        );
        // Past the probe a NUL does not count.
        let mut late = vec![b'a'; BINARY_PROBE + 10];
        late.push(0);
        late.extend_from_slice(b"\nneedle\n");
        assert!(matches!(
            scan(&late, "needle", false, 1000, 1000).await,
            ContentScan::Found(_)
        ));
        // Asked for, a binary file is searched.
        let m = ContentMatcher::new("needle", false, true).unwrap();
        let mut r = Trickle {
            data,
            at: 0,
            step: 7,
        };
        assert!(matches!(
            scan_content(&mut r, &m, true, 7).await.unwrap(),
            ContentScan::Found(_)
        ));
    }

    // SEARCH-004 — a file with no line breaks is searched in pieces, and a
    // match across the cut between pieces is found.
    #[tokio::test]
    async fn very_long_lines_are_searched_in_pieces() {
        let mut data = vec![b'a'; MAX_LINE + 100];
        let at = data.len();
        data.extend_from_slice(b"NEEDLE");
        data.extend(std::iter::repeat_n(b'b', MAX_LINE));
        match scan(&data, "needle", false, 64 * 1024, 64 * 1024).await {
            ContentScan::Found(s) => {
                assert_eq!(s.line, 1);
                assert!(s.text.contains("NEEDLE"), "{}", s.text);
                assert!(s.text.starts_with('…') && s.text.ends_with('…'));
                assert!(s.text.chars().count() <= SNIPPET_CHARS + 2);
            }
            other => panic!("{other:?} at {at}"),
        }
    }

    // SEARCH-005 — name masks: wildcards match whole names, plain text any
    // part, `;` separates masks, and case follows ignore_case.
    #[test]
    fn name_masks_and_regex() {
        let m = NameMatcher::new("*.rs; *.TOML", false, true).unwrap();
        assert!(m.matches("main.rs") && m.matches("Cargo.toml"));
        assert!(!m.matches("main.rs.bak") && !m.matches("readme.md"));
        let m = NameMatcher::new("ort", false, true).unwrap();
        assert!(m.matches("Report.txt"));
        let m = NameMatcher::new("Rep?rt*", false, false).unwrap();
        assert!(m.matches("Report.txt") && !m.matches("report.txt"));
        let m = NameMatcher::new("a+b[1].txt", false, true).unwrap();
        assert!(m.matches("x a+b[1].txt") && !m.matches("aab1.txt"));
        let m = NameMatcher::new(r"^report-\d{4}", true, true).unwrap();
        assert!(m.matches("Report-2025.md") && !m.matches("my report-2025.md"));
        assert!(NameMatcher::new("", false, true)
            .unwrap()
            .matches("anything"));
        let err = NameMatcher::new("(", true, true).unwrap_err();
        assert!(
            matches!(err, Error::Invalid(ref s) if s.starts_with("name pattern")),
            "{err:?}"
        );
        assert!(ContentMatcher::new("[", true, true).is_err());
        assert!(ContentMatcher::new("[", false, true).is_ok());
    }

    #[test]
    fn snippet_is_trimmed_and_shows_the_match() {
        let s = snippet(b"\t  let x = 1;  ", 5, 9);
        assert_eq!(
            s,
            Snippet {
                line: 9,
                text: "let x = 1;".into()
            }
        );
        // Invalid UTF-8 does not break it.
        let s = snippet(b"caf\xe9 needle", 5, 1);
        assert!(s.text.ends_with("needle"));
    }
}

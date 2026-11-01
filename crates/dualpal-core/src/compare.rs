//! Folder comparison: walk two folders (on any file systems) side by side
//! and classify every item.
//!
//! Items are reported in tree order (a folder, then its contents, folders
//! before files, by name) with a `/`-separated path relative to the
//! compared folders, so the UI can draw them as an indented tree and the
//! sync engine can address them on both sides.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{Entry, EntryKind, FileSystem, Location, Registry};

/// How two files with the same name are judged equal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareBy {
    /// Same size and modification time (within `tolerance_ms`).
    #[default]
    SizeAndDate,
    /// Same size; dates are only used to say which side is newer.
    Size,
    /// Same bytes. Reads both files when their sizes match.
    Content,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompareOptions {
    pub by: CompareBy,
    /// Modification times this close count as equal. FAT drives store
    /// times in 2 second steps and many servers in whole seconds.
    pub tolerance_ms: i64,
    /// Name patterns to leave out, e.g. `.DS_Store` or `*.tmp`
    /// (`*` and `?` wildcards, case-insensitive).
    pub exclude: Vec<String>,
    /// Include hidden items (names starting with a dot, or marked hidden).
    pub include_hidden: bool,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            by: CompareBy::SizeAndDate,
            tolerance_ms: 2000,
            exclude: vec![
                ".DS_Store".into(),
                "Thumbs.db".into(),
                "desktop.ini".into(),
                "._*".into(),
            ],
            include_hidden: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffState {
    Same,
    LeftOnly,
    RightOnly,
    LeftNewer,
    RightNewer,
    /// Different, but neither side is newer (same date, other size or
    /// content). For folders: something inside differs.
    Different,
    /// A file on one side and a folder on the other.
    KindMismatch,
}

/// What one side holds at a relative path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

impl Side {
    fn of(e: &Entry) -> Self {
        Self {
            dir: e.is_dir(),
            size: if e.is_dir() { 0 } else { e.size },
            modified_ms: e.modified_ms,
        }
    }

    /// The same file as `other` as far as sizes and dates go. Folders are
    /// the same folder whenever both are folders.
    pub fn matches(&self, other: &Side, tolerance_ms: i64) -> bool {
        if self.dir || other.dir {
            return self.dir == other.dir;
        }
        self.size == other.size && dates_equal(self.modified_ms, other.modified_ms, tolerance_ms)
    }
}

fn dates_equal(a: Option<i64>, b: Option<i64>, tolerance_ms: i64) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= tolerance_ms,
        _ => true,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffItem {
    /// Path relative to the compared folders, `/`-separated.
    pub rel: String,
    pub name: String,
    /// 0 for items directly in the compared folders.
    pub depth: u32,
    pub dir: bool,
    pub left: Option<Side>,
    pub right: Option<Side>,
    pub state: DiffState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareSummary {
    pub same: u64,
    pub left_only: u64,
    pub right_only: u64,
    pub left_newer: u64,
    pub right_newer: u64,
    pub different: u64,
    pub kind_mismatch: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub left: Location,
    pub right: Location,
    pub options: CompareOptions,
    pub items: Vec<DiffItem>,
    /// Counts of files (folders are not counted).
    pub summary: CompareSummary,
}

impl Comparison {
    pub fn get(&self, rel: &str) -> Option<&DiffItem> {
        self.items.iter().find(|i| i.rel == rel)
    }
}

/// Reports how far a comparison has got: folders read so far and the
/// folder being read.
pub type ProgressFn = Arc<dyn Fn(u64, &str) + Send + Sync>;

/// Turn a relative path into a path on `fs` below `root`.
pub fn resolve(fs: &dyn FileSystem, root: &str, rel: &str) -> String {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .fold(root.to_string(), |p, seg| fs.join(&p, seg))
}

/// Case-insensitive match of a name against a pattern with `*` and `?`.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

struct Walker {
    lfs: Arc<dyn FileSystem>,
    rfs: Arc<dyn FileSystem>,
    opts: CompareOptions,
    cancel: CancellationToken,
    progress: Option<ProgressFn>,
    scanned: u64,
    items: Vec<DiffItem>,
}

/// Compare the folders `left` and `right`.
pub async fn compare(
    registry: &Registry,
    left: &Location,
    right: &Location,
    opts: CompareOptions,
    cancel: CancellationToken,
    progress: Option<ProgressFn>,
) -> Result<Comparison> {
    let lfs = registry.get(&left.fs)?;
    let rfs = registry.get(&right.fs)?;
    for (fs, loc) in [(&lfs, left), (&rfs, right)] {
        if !fs.stat(&loc.path).await?.is_dir() {
            return Err(Error::Invalid(format!("not a folder: {}", loc.path)));
        }
    }
    if left.fs == right.fs
        && (lfs.is_within(&left.path, &right.path) || lfs.is_within(&right.path, &left.path))
    {
        return Err(Error::Invalid(
            "one folder is inside the other; compare two separate folders".into(),
        ));
    }
    let mut w = Walker {
        lfs,
        rfs,
        opts: opts.clone(),
        cancel,
        progress,
        scanned: 0,
        items: Vec::new(),
    };
    w.walk(
        Some(left.path.clone()),
        Some(right.path.clone()),
        String::new(),
        0,
    )
    .await?;
    let mut summary = CompareSummary::default();
    for i in w
        .items
        .iter()
        .filter(|i| !i.dir || i.state == DiffState::KindMismatch)
    {
        let n = match i.state {
            DiffState::Same => &mut summary.same,
            DiffState::LeftOnly => &mut summary.left_only,
            DiffState::RightOnly => &mut summary.right_only,
            DiffState::LeftNewer => &mut summary.left_newer,
            DiffState::RightNewer => &mut summary.right_newer,
            DiffState::Different => &mut summary.different,
            DiffState::KindMismatch => &mut summary.kind_mismatch,
        };
        *n += 1;
    }
    Ok(Comparison {
        left: left.clone(),
        right: right.clone(),
        options: opts,
        items: w.items,
        summary,
    })
}

/// Folders first, then by name without case, then by exact name.
type SortKey = (bool, String, String);
/// The entry of the same name on each side.
type Pair = (Option<Entry>, Option<Entry>);

type Walk<'a> = Pin<Box<dyn Future<Output = Result<bool>> + Send + 'a>>;

impl Walker {
    fn keep(&self, e: &Entry) -> bool {
        if !self.opts.include_hidden && (e.hidden || e.name.starts_with('.')) {
            return false;
        }
        if e.kind == EntryKind::Other {
            return false;
        }
        !self
            .opts
            .exclude
            .iter()
            .any(|p| glob_match(p.trim(), &e.name))
    }

    async fn list(&mut self, fs: &Arc<dyn FileSystem>, dir: &str) -> Result<Vec<Entry>> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.scanned += 1;
        if let Some(p) = &self.progress {
            p(self.scanned, dir);
        }
        let entries = fs.list(dir).await?;
        Ok(entries.into_iter().filter(|e| self.keep(e)).collect())
    }

    /// Compare the folder at `rel` (present on the sides given) and append
    /// its contents. Returns true if everything inside is the same.
    fn walk(&mut self, l: Option<String>, r: Option<String>, rel: String, depth: u32) -> Walk<'_> {
        Box::pin(async move {
            let lfs = self.lfs.clone();
            let rfs = self.rfs.clone();
            let left = match &l {
                Some(p) => self.list(&lfs, p).await?,
                None => Vec::new(),
            };
            let right = match &r {
                Some(p) => self.list(&rfs, p).await?,
                None => Vec::new(),
            };
            // name -> (left, right); sorted folders first, then by name.
            let mut pairs: BTreeMap<SortKey, Pair> = BTreeMap::new();
            let key = |e: &Entry, other: Option<&Entry>| {
                // A name that is a folder on either side sorts as a folder.
                let dir = e.is_dir() || other.is_some_and(|o| o.is_dir());
                (!dir, e.name.to_lowercase(), e.name.clone())
            };
            let right_by_name: std::collections::HashMap<&str, &Entry> =
                right.iter().map(|e| (e.name.as_str(), e)).collect();
            let left_names: std::collections::HashSet<&str> =
                left.iter().map(|e| e.name.as_str()).collect();
            for e in &left {
                let other = right_by_name.get(e.name.as_str()).copied();
                pairs.insert(key(e, other), (Some(e.clone()), other.cloned()));
            }
            for e in right
                .iter()
                .filter(|e| !left_names.contains(e.name.as_str()))
            {
                pairs.insert(key(e, None), (None, Some(e.clone())));
            }

            let mut all_same = true;
            for ((_, _, name), (le, re)) in pairs {
                let child_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let ls = le.as_ref().map(Side::of);
                let rs = re.as_ref().map(Side::of);
                // Symlinked folders are listed but not entered, to avoid cycles.
                let enter = |e: &Option<Entry>| {
                    e.as_ref()
                        .filter(|e| e.is_dir() && !e.symlink)
                        .map(|e| e.path.clone())
                };
                let index = self.items.len();
                self.items.push(DiffItem {
                    rel: child_rel.clone(),
                    name,
                    depth,
                    dir: ls.or(rs).is_some_and(|s| s.dir),
                    left: ls,
                    right: rs,
                    state: DiffState::Same,
                });
                let state = match (&ls, &rs) {
                    (Some(a), Some(b)) if a.dir != b.dir => DiffState::KindMismatch,
                    (Some(a), Some(_)) if a.dir => {
                        let same = self
                            .walk(enter(&le), enter(&re), child_rel, depth + 1)
                            .await?;
                        if same {
                            DiffState::Same
                        } else {
                            DiffState::Different
                        }
                    }
                    (Some(a), Some(b)) => {
                        self.file_state(a, b, le.as_ref().unwrap(), re.as_ref().unwrap())
                            .await?
                    }
                    (Some(a), None) => {
                        if a.dir {
                            self.walk(enter(&le), None, child_rel, depth + 1).await?;
                        }
                        DiffState::LeftOnly
                    }
                    (None, Some(b)) => {
                        if b.dir {
                            self.walk(None, enter(&re), child_rel, depth + 1).await?;
                        }
                        DiffState::RightOnly
                    }
                    (None, None) => unreachable!(),
                };
                if state != DiffState::Same {
                    all_same = false;
                }
                self.items[index].state = state;
            }
            Ok(all_same)
        })
    }

    async fn file_state(&self, a: &Side, b: &Side, le: &Entry, re: &Entry) -> Result<DiffState> {
        let tol = self.opts.tolerance_ms;
        let newer = || match (a.modified_ms, b.modified_ms) {
            (Some(x), Some(y)) if x - y > tol => DiffState::LeftNewer,
            (Some(x), Some(y)) if y - x > tol => DiffState::RightNewer,
            _ => DiffState::Different,
        };
        let same = match self.opts.by {
            CompareBy::SizeAndDate => a.matches(b, tol),
            CompareBy::Size => a.size == b.size,
            CompareBy::Content => a.size == b.size && self.same_content(&le.path, &re.path).await?,
        };
        Ok(if same { DiffState::Same } else { newer() })
    }

    async fn same_content(&self, l: &str, r: &str) -> Result<bool> {
        const CHUNK: usize = 256 * 1024;
        let mut a = self.lfs.open_read(l).await?;
        let mut b = self.rfs.open_read(r).await?;
        let mut ba = vec![0u8; CHUNK];
        let mut bb = vec![0u8; CHUNK];
        loop {
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let n = read_full(&mut a, &mut ba)
                .await
                .map_err(|e| Error::io(l, e))?;
            let m = read_full(&mut b, &mut bb)
                .await
                .map_err(|e| Error::io(r, e))?;
            if n != m || ba[..n] != bb[..m] {
                return Ok(false);
            }
            if n == 0 {
                return Ok(true);
            }
        }
    }
}

/// Fill `buf` as far as the stream allows (short only at the end).
async fn read_full(r: &mut crate::vfs::Reader, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn globs() {
        assert!(glob_match("*.tmp", "a.TMP"));
        assert!(glob_match(".DS_Store", ".ds_store"));
        assert!(glob_match("._*", "._foo"));
        assert!(!glob_match("._*", "foo"));
        assert!(glob_match("a?c*", "abcdef"));
        assert!(!glob_match("a?c", "abcd"));
        assert!(glob_match("*", ""));
    }
}

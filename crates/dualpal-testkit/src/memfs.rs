//! An in-memory [`FileSystem`] with POSIX-style paths.
//!
//! It is the fast backend for the VFS contract tests and for sync/compare
//! tests that need a "remote" side without a server. Semantics follow a
//! normal Unix disk: `/` is the root, names are case-sensitive, `.name` is
//! hidden, a rename replaces an existing file, and `open_write` truncates.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use dualpal_core::vfs::{Entry, EntryKind, FileSystem, Reader, Writer};
use dualpal_core::{Error, Result};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A file system that lives in a map. Cheap to clone; all clones share it.
#[derive(Clone)]
pub struct MemFs {
    inner: Arc<Mutex<Inner>>,
    id: String,
    parallel: usize,
}

struct Inner {
    nodes: BTreeMap<String, Node>,
}

struct Node {
    kind: EntryKind,
    data: Vec<u8>,
    mode: u32,
    modified_ms: i64,
    created_ms: i64,
    /// Where a symlink points, if this node is one.
    symlink: Option<String>,
}

impl MemFs {
    pub fn new(id: impl Into<String>) -> Self {
        let now = dualpal_core::now_ms();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "/".to_string(),
            Node {
                kind: EntryKind::Dir,
                data: Vec::new(),
                mode: 0o755,
                modified_ms: now,
                created_ms: now,
                symlink: None,
            },
        );
        Self {
            inner: Arc::new(Mutex::new(Inner { nodes })),
            id: id.into(),
            parallel: 2,
        }
    }

    /// A backend registered as `"mem"`.
    pub fn with_default_id() -> Self {
        Self::new("mem")
    }

    /// How many files this fake lets the operation queue copy at once.
    pub fn set_parallel_transfers(&mut self, n: usize) {
        self.parallel = n.max(1);
    }

    /// Create `path` as a folder, failing if its parent is missing.
    pub fn seed_dir(&self, path: &str) {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let parent = parent_of(&path).expect("root always exists");
        assert!(
            inner
                .nodes
                .get(&parent)
                .is_some_and(|n| n.kind == EntryKind::Dir),
            "parent {parent} does not exist"
        );
        let now = dualpal_core::now_ms();
        inner.nodes.insert(
            path,
            Node {
                kind: EntryKind::Dir,
                data: Vec::new(),
                mode: 0o755,
                modified_ms: now,
                created_ms: now,
                symlink: None,
            },
        );
    }

    /// Create `path` with `content`, creating missing parent folders.
    pub fn seed_file(&self, path: &str, content: impl AsRef<[u8]>) {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let mut parent = parent_of(&path).unwrap_or_else(|| "/".to_string());
        let mut missing = Vec::new();
        loop {
            if inner.nodes.contains_key(&parent) {
                break;
            }
            missing.push(parent.clone());
            match parent_of(&parent) {
                Some(p) => parent = p,
                None => break,
            }
        }
        let now = dualpal_core::now_ms();
        for dir in missing.into_iter().rev() {
            inner.nodes.insert(
                dir,
                Node {
                    kind: EntryKind::Dir,
                    data: Vec::new(),
                    mode: 0o755,
                    modified_ms: now,
                    created_ms: now,
                    symlink: None,
                },
            );
        }
        inner.nodes.insert(
            path,
            Node {
                kind: EntryKind::File,
                data: content.as_ref().to_vec(),
                mode: 0o644,
                modified_ms: now,
                created_ms: now,
                symlink: None,
            },
        );
    }

    /// Create a symlink node. Most contract runs do not use this; it is here
    /// for backend-specific tests and for exercising the listing code.
    pub fn seed_symlink(&self, path: &str, target: &str) {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let now = dualpal_core::now_ms();
        inner.nodes.insert(
            path,
            Node {
                kind: EntryKind::Other,
                data: Vec::new(),
                mode: 0o777,
                modified_ms: now,
                created_ms: now,
                symlink: Some(normalize(target)),
            },
        );
    }

    /// The bytes at `path`, if it is a file.
    pub fn contents(&self, path: &str) -> Option<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
        let node = inner.nodes.get(&normalize(path))?;
        (node.kind == EntryKind::File).then(|| node.data.clone())
    }

    /// Every path in the fake, sorted. Handy for assertions after a sync.
    pub fn paths(&self) -> Vec<String> {
        self.inner.lock().unwrap().nodes.keys().cloned().collect()
    }

    /// Remove everything below `path` (or `path` itself), for a fresh start.
    pub fn wipe(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.nodes.retain(|p, _| p == "/");
    }
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn parent_of(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    match path.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(path[..i].to_string()),
        None => None,
    }
}

fn name_of(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }
    path.rsplit('/').next().map(|s| s.to_string())
}

impl Inner {
    /// Follow symlinks up to a small depth; `None` when the link dangles.
    fn resolve(&self, path: &str) -> Option<(&Node, String)> {
        let mut current = normalize(path);
        for _ in 0..16 {
            let node = self.nodes.get(&current)?;
            match &node.symlink {
                Some(target) => current = normalize(target),
                None => return Some((node, current)),
            }
        }
        None
    }

    fn entry(&self, path: &str) -> Result<Entry> {
        let path = normalize(path);
        let Some(link) = self.nodes.get(&path) else {
            return Err(Error::NotFound(path));
        };
        let symlink = link.symlink.is_some();
        let (kind, size, mode, modified_ms, created_ms) = match self.resolve(&path) {
            Some((node, _)) => (
                node.kind,
                if node.kind == EntryKind::File {
                    node.data.len() as u64
                } else {
                    0
                },
                node.mode,
                node.modified_ms,
                node.created_ms,
            ),
            // A dangling link is still listed, as an "other" item.
            None => (
                EntryKind::Other,
                0,
                link.mode,
                link.modified_ms,
                link.created_ms,
            ),
        };
        let name = name_of(&path).unwrap_or_else(|| path.clone());
        Ok(Entry {
            hidden: name.starts_with('.'),
            name,
            path,
            kind,
            symlink,
            size,
            modified_ms: Some(modified_ms),
            created_ms: Some(created_ms),
            readonly: mode & 0o222 == 0,
            mode: Some(mode),
        })
    }
}

#[async_trait]
impl FileSystem for MemFs {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        format!("{} (memory)", self.id)
    }

    fn join(&self, dir: &str, name: &str) -> String {
        normalize(&format!("{dir}/{name}"))
    }

    fn parent(&self, path: &str) -> Option<String> {
        parent_of(&normalize(path))
    }

    fn file_name(&self, path: &str) -> Option<String> {
        name_of(&normalize(path))
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        let (path, ancestor) = (normalize(path), normalize(ancestor));
        path == ancestor
            || (ancestor == "/" && path.starts_with('/'))
            || path.starts_with(&format!("{ancestor}/"))
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let inner = self.inner.lock().unwrap();
        let dir = normalize(dir);
        let Some((node, real)) = inner.resolve(&dir) else {
            return Err(Error::NotFound(dir));
        };
        if node.kind != EntryKind::Dir {
            return Err(Error::Invalid(format!("not a folder: {dir}")));
        }
        let mut out = Vec::new();
        for path in inner.nodes.keys() {
            if path != &real && parent_of(path).as_deref() == Some(real.as_str()) {
                out.push(inner.entry(path)?);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        self.inner.lock().unwrap().entry(path)
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        if inner.nodes.contains_key(&path) {
            return Err(Error::AlreadyExists(path));
        }
        let parent = parent_of(&path).ok_or_else(|| Error::AlreadyExists(path.clone()))?;
        match inner.nodes.get(&parent) {
            Some(n) if n.kind == EntryKind::Dir => {}
            _ => return Err(Error::NotFound(parent)),
        }
        let now = dualpal_core::now_ms();
        inner.nodes.insert(
            path,
            Node {
                kind: EntryKind::Dir,
                data: Vec::new(),
                mode: 0o755,
                modified_ms: now,
                created_ms: now,
                symlink: None,
            },
        );
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        match inner.nodes.get(&path) {
            None => Err(Error::NotFound(path)),
            Some(n) if n.kind == EntryKind::Dir && n.symlink.is_none() => Err(Error::Invalid(
                format!("{path} is a folder; use remove_dir"),
            )),
            Some(_) => {
                inner.nodes.remove(&path);
                Ok(())
            }
        }
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        match inner.nodes.get(&path) {
            None => Err(Error::NotFound(path)),
            Some(n) if n.kind != EntryKind::Dir || n.symlink.is_some() => {
                Err(Error::Invalid(format!("{path} is not a folder")))
            }
            Some(_) => {
                if inner
                    .nodes
                    .keys()
                    .any(|p| p != &path && p.starts_with(&format!("{path}/")))
                {
                    return Err(Error::Invalid(format!("folder is not empty: {path}")));
                }
                inner.nodes.remove(&path);
                Ok(())
            }
        }
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let from = normalize(from);
        let to = normalize(to);
        let source_is_dir = match inner.nodes.get(&from) {
            Some(n) => n.kind == EntryKind::Dir,
            None => return Err(Error::NotFound(from.clone())),
        };
        let to_is_inside = to == from || from == "/" || to.starts_with(&format!("{from}/"));
        if source_is_dir && to_is_inside {
            return Err(Error::Invalid(format!(
                "cannot move a folder inside itself: {from}"
            )));
        }
        let parent = parent_of(&to).ok_or_else(|| Error::AlreadyExists(to.clone()))?;
        match inner.nodes.get(&parent) {
            Some(n) if n.kind == EntryKind::Dir => {}
            _ => return Err(Error::NotFound(parent)),
        }
        if let Some(existing) = inner.nodes.get(&to) {
            let target_is_dir = existing.kind == EntryKind::Dir;
            if target_is_dir != source_is_dir {
                return Err(Error::AlreadyExists(to));
            }
            if target_is_dir
                && inner
                    .nodes
                    .keys()
                    .any(|p| p != &to && p.starts_with(&format!("{to}/")))
            {
                return Err(Error::AlreadyExists(to));
            }
            inner.nodes.remove(&to);
        }
        let moved: Vec<(String, Node)> = {
            let prefix = format!("{from}/");
            let keys: Vec<String> = inner
                .nodes
                .keys()
                .filter(|p| *p == &from || p.starts_with(&prefix))
                .cloned()
                .collect();
            keys.into_iter()
                .map(|p| {
                    let node = inner.nodes.remove(&p).unwrap();
                    (p, node)
                })
                .collect()
        };
        for (old, node) in moved {
            let new = if old == from {
                to.clone()
            } else {
                format!("{to}{}", &old[from.len()..])
            };
            inner.nodes.insert(new, node);
        }
        Ok(())
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        let inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let Some((node, real)) = inner.resolve(&path) else {
            return Err(Error::NotFound(path));
        };
        if node.kind != EntryKind::File {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        Ok(Box::new(MemReader {
            inner: self.inner.clone(),
            path: real,
            pos: 0,
        }))
    }

    async fn open_read_at(&self, path: &str, offset: u64) -> Result<Reader> {
        let inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let Some((node, real)) = inner.resolve(&path) else {
            return Err(Error::NotFound(path));
        };
        if node.kind != EntryKind::File {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        if offset > node.data.len() as u64 {
            return Err(Error::Invalid(format!(
                "{path} is shorter than {offset} bytes"
            )));
        }
        Ok(Box::new(MemReader {
            inner: self.inner.clone(),
            path: real,
            pos: offset as usize,
        }))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let parent = parent_of(&path).ok_or_else(|| Error::AlreadyExists(path.clone()))?;
        match inner.nodes.get(&parent) {
            Some(n) if n.kind == EntryKind::Dir => {}
            _ => return Err(Error::NotFound(parent)),
        }
        let mode = match inner.nodes.get(&path) {
            Some(n) if n.kind == EntryKind::Dir => {
                return Err(Error::Invalid(format!("{path} is a folder")))
            }
            Some(n) => n.mode,
            None => 0o644,
        };
        let now = dualpal_core::now_ms();
        let created_ms = inner.nodes.get(&path).map(|n| n.created_ms).unwrap_or(now);
        inner.nodes.insert(
            path.clone(),
            Node {
                kind: EntryKind::File,
                data: Vec::new(),
                mode,
                modified_ms: now,
                created_ms,
                symlink: None,
            },
        );
        Ok(Box::new(MemWriter {
            inner: self.inner.clone(),
            path,
            pos: 0,
        }))
    }

    async fn open_write_at(&self, path: &str, offset: u64) -> Result<Writer> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        let Some(node) = inner.nodes.get_mut(&path) else {
            return Err(Error::NotFound(path));
        };
        if node.kind != EntryKind::File {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        node.data.resize(offset as usize, 0);
        node.modified_ms = dualpal_core::now_ms();
        Ok(Box::new(MemWriter {
            inner: self.inner.clone(),
            path,
            pos: offset as usize,
        }))
    }

    fn resumable_writes(&self) -> bool {
        true
    }

    fn parallel_transfers(&self) -> usize {
        self.parallel
    }

    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        match inner.nodes.get_mut(&path) {
            Some(n) => {
                n.mode = mode & 0o7777;
                Ok(())
            }
            None => Err(Error::NotFound(path)),
        }
    }

    async fn set_modified(&self, path: &str, modified_ms: i64) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let path = normalize(path);
        match inner.nodes.get_mut(&path) {
            Some(n) => {
                n.modified_ms = modified_ms;
                Ok(())
            }
            None => Err(Error::NotFound(path)),
        }
    }

    async fn trash(&self, path: &str) -> Result<()> {
        Err(Error::Unsupported(format!(
            "this location has no Trash: {path}"
        )))
    }
}

struct MemReader {
    inner: Arc<Mutex<Inner>>,
    path: String,
    pos: usize,
}

impl AsyncRead for MemReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let inner = this.inner.lock().unwrap();
        let Some(node) = inner.nodes.get(&this.path) else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                this.path.clone(),
            )));
        };
        let rest = &node.data[this.pos.min(node.data.len())..];
        let n = rest.len().min(buf.remaining());
        buf.put_slice(&rest[..n]);
        this.pos += n;
        Poll::Ready(Ok(()))
    }
}

struct MemWriter {
    inner: Arc<Mutex<Inner>>,
    path: String,
    pos: usize,
}

impl AsyncWrite for MemWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let mut inner = this.inner.lock().unwrap();
        let Some(node) = inner.nodes.get_mut(&this.path) else {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                this.path.clone(),
            )));
        };
        if node.kind != EntryKind::File {
            return Poll::Ready(Err(std::io::Error::other("not a file")));
        }
        if node.data.len() < this.pos {
            node.data.resize(this.pos, 0);
        }
        let end = this.pos + buf.len();
        if node.data.len() < end {
            node.data.resize(end, 0);
        }
        node.data[this.pos..end].copy_from_slice(buf);
        this.pos = end;
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

//! Virtual file system abstraction.
//!
//! Every place the app can browse (local disk, an SFTP server, an S3 bucket)
//! is a [`FileSystem`] registered under an id in the [`Registry`]. A
//! [`Location`] is a pair of that id and a path inside it, so the UI, the
//! operation queue and (later) the sync engine can treat all of them alike.

mod local;
mod text;

pub use local::LocalFs;
pub use text::{read_text, write_text, TextFile};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::{Error, Result};

/// A path inside a specific registered file system.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// Id of the file system in the [`Registry`], e.g. `"local"`.
    pub fs: String,
    /// Path in that file system's native syntax.
    pub path: String,
}

impl Location {
    pub fn new(fs: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            fs: fs.into(),
            path: path.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Dir,
    Other,
}

/// One item in a directory listing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    /// Kind of the entry. For symlinks this is the kind of the link target.
    pub kind: EntryKind,
    /// The entry itself is a symbolic link.
    pub symlink: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub hidden: bool,
    pub readonly: bool,
    /// Unix permission bits, when the file system has them.
    pub mode: Option<u32>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }
}

pub type Reader = Box<dyn AsyncRead + Send + Unpin>;
pub type Writer = Box<dyn AsyncWrite + Send + Unpin>;

/// A browsable storage backend.
///
/// Paths are strings in the backend's native syntax; use [`FileSystem::join`]
/// and [`FileSystem::parent`] rather than manipulating them directly.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Id under which this file system is registered.
    fn id(&self) -> &str;
    /// Human-readable name, e.g. "This Mac" or "my-bucket (S3)".
    fn display_name(&self) -> String;

    fn join(&self, dir: &str, name: &str) -> String;
    fn parent(&self, path: &str) -> Option<String>;
    fn file_name(&self, path: &str) -> Option<String>;
    /// True if `path` is `ancestor` or lies somewhere below it.
    fn is_within(&self, path: &str, ancestor: &str) -> bool;

    async fn list(&self, dir: &str) -> Result<Vec<Entry>>;
    async fn stat(&self, path: &str) -> Result<Entry>;
    async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
    async fn create_dir(&self, path: &str) -> Result<()>;
    /// Remove a file or a symlink.
    async fn remove_file(&self, path: &str) -> Result<()>;
    /// Remove an empty directory.
    async fn remove_dir(&self, path: &str) -> Result<()>;
    async fn rename(&self, from: &str, to: &str) -> Result<()>;
    async fn open_read(&self, path: &str) -> Result<Reader>;
    /// Create or truncate a file for writing.
    async fn open_write(&self, path: &str) -> Result<Writer>;
    /// Move an item to the system Trash / Recycle Bin.
    async fn trash(&self, path: &str) -> Result<()> {
        Err(Error::Unsupported(format!(
            "this location has no Trash: {path}"
        )))
    }
    /// Change Unix permission bits.
    async fn set_mode(&self, path: &str, _mode: u32) -> Result<()> {
        Err(Error::Unsupported(format!(
            "changing permissions here: {path}"
        )))
    }
    /// Set the modification time. Backends that cannot do this may ignore it.
    async fn set_modified(&self, _path: &str, _modified_ms: i64) -> Result<()> {
        Ok(())
    }
}

/// All file systems the app currently knows about, keyed by id.
#[derive(Default)]
pub struct Registry {
    map: RwLock<HashMap<String, Arc<dyn FileSystem>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with the local disk registered as `"local"`.
    pub fn with_local() -> Self {
        let r = Self::new();
        r.register(Arc::new(LocalFs::new()));
        r
    }

    pub fn register(&self, fs: Arc<dyn FileSystem>) {
        self.map.write().unwrap().insert(fs.id().to_string(), fs);
    }

    pub fn unregister(&self, id: &str) -> Option<Arc<dyn FileSystem>> {
        self.map.write().unwrap().remove(id)
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn FileSystem>> {
        self.map
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::UnknownFileSystem(id.to_string()))
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.map.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }
}

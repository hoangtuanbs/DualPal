//! The local disk backend.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;

use super::{Entry, EntryKind, FileSystem, Reader, Writer};
use crate::error::{Error, Result};

pub struct LocalFs {
    id: String,
}

impl LocalFs {
    pub const ID: &'static str = "local";

    pub fn new() -> Self {
        Self {
            id: Self::ID.to_string(),
        }
    }
}

impl Default for LocalFs {
    fn default() -> Self {
        Self::new()
    }
}

fn to_ms(t: std::io::Result<SystemTime>) -> Option<i64> {
    t.ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn is_hidden(name: &str, meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    name.starts_with('.') || meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn is_hidden(name: &str, _meta: &Metadata) -> bool {
    name.starts_with('.')
}

#[cfg(unix)]
fn mode(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode(_meta: &Metadata) -> Option<u32> {
    None
}

/// Build an entry from `symlink_metadata`, following the link (if any) to
/// learn the target's kind and size.
async fn entry_for(path: &Path, link_meta: Metadata) -> Entry {
    let symlink = link_meta.file_type().is_symlink();
    let meta = if symlink {
        tokio::fs::metadata(path).await.unwrap_or(link_meta)
    } else {
        link_meta
    };
    let ft = meta.file_type();
    let kind = if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_str(path));
    Entry {
        hidden: is_hidden(&name, &meta),
        name,
        path: path_str(path),
        kind,
        symlink,
        size: if kind == EntryKind::File {
            meta.len()
        } else {
            0
        },
        modified_ms: to_ms(meta.modified()),
        created_ms: to_ms(meta.created()),
        readonly: meta.permissions().readonly(),
        mode: mode(&meta),
    }
}

#[async_trait]
impl FileSystem for LocalFs {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        if cfg!(target_os = "macos") {
            "This Mac".into()
        } else {
            "This Computer".into()
        }
    }

    fn join(&self, dir: &str, name: &str) -> String {
        path_str(&Path::new(dir).join(name))
    }

    fn parent(&self, path: &str) -> Option<String> {
        Path::new(path).parent().map(path_str)
    }

    fn file_name(&self, path: &str) -> Option<String> {
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        Path::new(path).starts_with(Path::new(ancestor))
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let mut rd = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| Error::io(dir, e))?;
        let mut out = Vec::new();
        while let Some(de) = rd.next_entry().await.map_err(|e| Error::io(dir, e))? {
            let path: PathBuf = de.path();
            // An entry can vanish between read_dir and stat; skip it.
            let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
                continue;
            };
            out.push(entry_for(&path, meta).await);
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        let meta = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|e| Error::io(path, e))?;
        Ok(entry_for(Path::new(path), meta).await)
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        tokio::fs::create_dir(path)
            .await
            .map_err(|e| Error::io(path, e))
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            // On Windows a symlink to a directory must be removed with remove_dir.
            Err(e) if cfg!(windows) => tokio::fs::remove_dir(path)
                .await
                .map_err(|_| Error::io(path, e)),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        tokio::fs::remove_dir(path)
            .await
            .map_err(|e| Error::io(path, e))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        tokio::fs::rename(from, to)
            .await
            .map_err(|e| Error::io(from, e))
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        let f = tokio::fs::File::open(path)
            .await
            .map_err(|e| Error::io(path, e))?;
        Ok(Box::new(f))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        let f = tokio::fs::File::create(path)
            .await
            .map_err(|e| Error::io(path, e))?;
        Ok(Box::new(f))
    }

    async fn trash(&self, path: &str) -> Result<()> {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || {
            if std::fs::symlink_metadata(&path).is_err() {
                return Err(Error::NotFound(path));
            }
            trash::delete(&path).map_err(|e| Error::Io {
                path: path.clone(),
                source: std::io::Error::other(e.to_string()),
            })
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
    }

    async fn set_modified(&self, path: &str, modified_ms: i64) -> Result<()> {
        let path = path.to_string();
        tokio::task::spawn_blocking(move || {
            let t = SystemTime::UNIX_EPOCH
                + std::time::Duration::from_millis(modified_ms.max(0) as u64);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_modified(t))
                .map_err(|e| Error::io(&path, e))
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
    }
}

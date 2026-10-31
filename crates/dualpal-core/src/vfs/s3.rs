//! Amazon S3 and S3-compatible storage (MinIO, Cloudflare R2, Wasabi,
//! Backblaze B2…), built on Apache OpenDAL.
//!
//! S3 has no real folders: a "folder" is a key prefix, plus an optional empty
//! `name/` marker object so that empty folders survive. Paths here look like
//! any other: `/photos/2024/a.jpg`, with `/` being the bucket root.

use std::time::SystemTime;

use async_trait::async_trait;
use opendal::{services, EntryMode, ErrorKind, Operator};
use tokio_util::compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

use super::{posix, Entry, EntryKind, FileSystem, Reader, Writer};
use crate::error::{Error, Result};

/// Size of each multipart upload part.
const PART_SIZE: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct S3Params {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub path_style: bool,
}

pub struct S3Fs {
    id: String,
    name: String,
    op: Operator,
}

/// `/a/b` → `a/b`; the root becomes `/`.
fn key(path: &str) -> String {
    let p = posix::normalize(path);
    let k = p.trim_start_matches('/');
    if k.is_empty() {
        "/".to_string()
    } else {
        k.to_string()
    }
}

/// `/a/b` → `a/b/`; the root stays `/`.
fn dir_key(path: &str) -> String {
    let k = key(path);
    if k == "/" {
        k
    } else {
        format!("{k}/")
    }
}

fn map_err(path: &str, e: opendal::Error) -> Error {
    match e.kind() {
        ErrorKind::NotFound => Error::NotFound(path.to_string()),
        ErrorKind::PermissionDenied => Error::PermissionDenied(path.to_string()),
        ErrorKind::AlreadyExists => Error::AlreadyExists(path.to_string()),
        ErrorKind::Unsupported => Error::Unsupported(format!("{path}: {e}")),
        _ => Error::Io {
            path: path.to_string(),
            source: std::io::Error::other(e.to_string()),
        },
    }
}

fn ms(t: Option<opendal::raw::Timestamp>) -> Option<i64> {
    let t: SystemTime = t?.into();
    t.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn dir_entry(path: String, name: String, modified_ms: Option<i64>) -> Entry {
    Entry {
        hidden: name.starts_with('.'),
        name,
        path,
        kind: EntryKind::Dir,
        symlink: false,
        size: 0,
        modified_ms,
        created_ms: None,
        readonly: false,
        mode: None,
    }
}

impl S3Fs {
    pub async fn connect(id: String, name: String, p: S3Params) -> Result<Self> {
        let mut b = services::S3::default()
            .bucket(&p.bucket)
            .region(if p.region.trim().is_empty() {
                "us-east-1"
            } else {
                p.region.trim()
            })
            .access_key_id(&p.access_key_id)
            .secret_access_key(&p.secret_access_key)
            // Only use the credentials given here, never ~/.aws or instance metadata.
            .disable_config_load()
            .disable_ec2_metadata();
        if let Some(ep) = &p.endpoint {
            b = b.endpoint(ep.trim());
        }
        if !p.path_style {
            b = b.enable_virtual_host_style();
        }
        let op = Operator::new(b)
            .map_err(|e| Error::Invalid(e.to_string()))?
            .finish();
        let fs = Self::from_operator(id, name, op);
        // Fail now, with a clear message, rather than on the first listing.
        match fs.op.list_with("/").limit(1).await {
            Ok(_) => Ok(fs),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => Err(Error::AuthFailed(format!(
                "S3 refused the access key for bucket {}",
                p.bucket
            ))),
            Err(e) if e.kind() == ErrorKind::NotFound => Err(Error::Connection(format!(
                "bucket {} was not found",
                p.bucket
            ))),
            Err(e) => Err(Error::Connection(e.to_string())),
        }
    }

    /// Wrap an existing operator (tests use the in-memory service).
    pub fn from_operator(id: String, name: String, op: Operator) -> Self {
        Self { id, name, op }
    }

    async fn is_dir(&self, path: &str) -> Result<bool> {
        let dk = dir_key(path);
        if dk == "/" {
            return Ok(true);
        }
        match self.op.list_with(&dk).limit(2).await {
            Ok(items) => Ok(!items.is_empty()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(map_err(path, e)),
        }
    }

    /// Server-side copy where the service supports it (S3 does), otherwise
    /// download and upload again.
    async fn copy_key(&self, from: &str, to: &str) -> Result<()> {
        if self.op.info().full_capability().copy {
            return self.op.copy(from, to).await.map_err(|e| map_err(from, e));
        }
        let data = self.op.read(from).await.map_err(|e| map_err(from, e))?;
        self.op.write(to, data).await.map_err(|e| map_err(to, e))?;
        Ok(())
    }

    /// All file keys under a folder, recursively, plus the folder markers.
    async fn walk(&self, path: &str) -> Result<Vec<String>> {
        let items = self
            .op
            .list_with(&dir_key(path))
            .recursive(true)
            .await
            .map_err(|e| map_err(path, e))?;
        Ok(items.into_iter().map(|e| e.path().to_string()).collect())
    }
}

#[async_trait]
impl FileSystem for S3Fs {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        self.name.clone()
    }

    fn join(&self, dir: &str, name: &str) -> String {
        posix::join(dir, name)
    }

    fn parent(&self, path: &str) -> Option<String> {
        posix::parent(path)
    }

    fn file_name(&self, path: &str) -> Option<String> {
        posix::file_name(path)
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        posix::is_within(path, ancestor)
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let dk = dir_key(dir);
        let items = self.op.list(&dk).await.map_err(|e| map_err(dir, e))?;
        if items.is_empty() && !self.is_dir(dir).await? {
            return Err(Error::NotFound(dir.to_string()));
        }
        let base = posix::normalize(dir);
        let mut out = Vec::new();
        for it in items {
            if it.path() == dk || it.path() == "/" {
                continue; // the folder's own marker
            }
            let name = it.name().trim_end_matches('/').to_string();
            if name.is_empty() {
                continue;
            }
            let path = posix::join(&base, &name);
            let meta = it.metadata();
            match meta.mode() {
                EntryMode::DIR => out.push(dir_entry(path, name, ms(meta.last_modified()))),
                _ => out.push(Entry {
                    hidden: name.starts_with('.'),
                    name,
                    path,
                    kind: EntryKind::File,
                    symlink: false,
                    size: meta.content_length(),
                    modified_ms: ms(meta.last_modified()),
                    created_ms: None,
                    readonly: false,
                    mode: None,
                }),
            }
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        let p = posix::normalize(path);
        let name = posix::file_name(&p).unwrap_or_else(|| "/".to_string());
        let k = key(&p);
        if k != "/" {
            match self.op.stat(&k).await {
                Ok(meta) if meta.mode() == EntryMode::FILE => {
                    return Ok(Entry {
                        hidden: name.starts_with('.'),
                        name,
                        path: p,
                        kind: EntryKind::File,
                        symlink: false,
                        size: meta.content_length(),
                        modified_ms: ms(meta.last_modified()),
                        created_ms: None,
                        readonly: false,
                        mode: None,
                    })
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(map_err(path, e)),
            }
        }
        if self.is_dir(&p).await? {
            Ok(dir_entry(p, name, None))
        } else {
            Err(Error::NotFound(path.to_string()))
        }
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        if self.exists(path).await? {
            return Err(Error::AlreadyExists(path.to_string()));
        }
        self.op
            .create_dir(&dir_key(path))
            .await
            .map_err(|e| map_err(path, e))
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.op
            .delete(&key(path))
            .await
            .map_err(|e| map_err(path, e))
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        let dk = dir_key(path);
        if dk == "/" {
            return Err(Error::Invalid("cannot remove the bucket root".into()));
        }
        let items = self
            .op
            .list_with(&dk)
            .limit(2)
            .await
            .map_err(|e| map_err(path, e))?;
        if items.iter().any(|e| e.path() != dk) {
            return Err(Error::Invalid(format!("folder is not empty: {path}")));
        }
        self.op.delete(&dk).await.map_err(|e| map_err(path, e))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // Like a local rename, replace an existing file but never a folder.
        let entry = self.stat(from).await?;
        match self.stat(to).await {
            Ok(t) if t.is_dir() || entry.is_dir() => {
                return Err(Error::AlreadyExists(to.to_string()))
            }
            Ok(_) | Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        if !entry.is_dir() {
            self.copy_key(&key(from), &key(to)).await?;
            return self
                .op
                .delete(&key(from))
                .await
                .map_err(|e| map_err(from, e));
        }
        // S3 cannot rename a prefix: copy every object, then delete the originals.
        let from_dk = dir_key(from);
        let to_dk = dir_key(to);
        let keys = self.walk(from).await?;
        self.op
            .create_dir(&to_dk)
            .await
            .map_err(|e| map_err(to, e))?;
        for k in &keys {
            let rest = &k[from_dk.len().min(k.len())..];
            if k.ends_with('/') {
                if !rest.is_empty() {
                    self.op
                        .create_dir(&format!("{to_dk}{rest}"))
                        .await
                        .map_err(|e| map_err(to, e))?;
                }
            } else {
                self.copy_key(k, &format!("{to_dk}{rest}")).await?;
            }
        }
        for k in keys.iter().filter(|k| !k.ends_with('/')) {
            self.op.delete(k).await.map_err(|e| map_err(k, e))?;
        }
        let _ = self.op.remove_all(&from_dk).await;
        Ok(())
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        let k = key(path);
        let size = self
            .op
            .stat(&k)
            .await
            .map_err(|e| map_err(path, e))?
            .content_length();
        let r = self
            .op
            .reader_with(&k)
            .chunk(PART_SIZE)
            .await
            .map_err(|e| map_err(path, e))?
            .into_futures_async_read(0..size)
            .await
            .map_err(|e| map_err(path, e))?;
        Ok(Box::new(r.compat()))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        let w = self
            .op
            .writer_with(&key(path))
            .chunk(PART_SIZE)
            .await
            .map_err(|e| map_err(path, e))?;
        Ok(Box::new(w.into_futures_async_write().compat_write()))
    }
}

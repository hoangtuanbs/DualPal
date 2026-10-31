//! Editing remote files in a local app.
//!
//! The file is downloaded to a private temporary folder and opened in the
//! user's editor (the front end does the opening). A watcher polls the local
//! copy and uploads it again every time it is saved, until the session is
//! stopped. Polling instead of OS file events keeps this identical on macOS
//! and Windows and copes with editors that save by replacing the file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::vfs::{FileSystem, Location, Registry};

const POLL: Duration = Duration::from_millis(800);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EditStatus {
    /// Waiting for the editor to save.
    Watching,
    Uploading,
    /// The last save was uploaded.
    Uploaded,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSession {
    pub id: Uuid,
    pub remote: Location,
    pub local_path: String,
    pub status: EditStatus,
    pub uploads: u32,
    pub last_upload_ms: Option<i64>,
    pub error: Option<String>,
}

struct Running {
    info: EditSession,
    cancel: CancellationToken,
}

#[derive(Clone)]
pub struct RemoteEditor {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Arc<Registry>,
    root: PathBuf,
    sessions: Mutex<HashMap<Uuid, Running>>,
    tx: broadcast::Sender<EditSession>,
}

/// Copy a file from any file system to a local path.
pub async fn download(fs: &dyn FileSystem, path: &str, to: &Path) -> Result<()> {
    if let Some(dir) = to.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::io(dir.to_string_lossy(), e))?;
    }
    let mut r = fs.open_read(path).await?;
    let mut w = tokio::fs::File::create(to)
        .await
        .map_err(|e| Error::io(to.to_string_lossy(), e))?;
    tokio::io::copy(&mut r, &mut w)
        .await
        .map_err(|e| Error::io(path, e))?;
    w.flush()
        .await
        .map_err(|e| Error::io(to.to_string_lossy(), e))
}

async fn upload(fs: &dyn FileSystem, from: &Path, path: &str) -> Result<()> {
    let mut r = tokio::fs::File::open(from)
        .await
        .map_err(|e| Error::io(from.to_string_lossy(), e))?;
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)
        .await
        .map_err(|e| Error::io(from.to_string_lossy(), e))?;
    let mut w = fs.open_write(path).await?;
    w.write_all(&buf).await.map_err(|e| Error::io(path, e))?;
    w.shutdown().await.map_err(|e| Error::io(path, e))
}

/// (modified time, size) of a local file, if it exists.
fn signature(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl RemoteEditor {
    /// `root` is a private folder for the downloaded copies.
    pub fn new(registry: Arc<Registry>, root: impl Into<PathBuf>) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                registry,
                root: root.into(),
                sessions: Mutex::new(HashMap::new()),
                tx,
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EditSession> {
        self.inner.tx.subscribe()
    }

    pub fn list(&self) -> Vec<EditSession> {
        let mut v: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|r| r.info.clone())
            .collect();
        v.sort_by(|a, b| a.local_path.cmp(&b.local_path));
        v
    }

    /// Download `remote` and start watching the copy. Returns the session,
    /// whose `local_path` the caller opens in the editor. Editing the same
    /// file twice reuses the running session.
    pub async fn start(&self, remote: Location) -> Result<EditSession> {
        if let Some(r) = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .find(|r| r.info.remote == remote)
        {
            return Ok(r.info.clone());
        }
        let fs = self.inner.registry.get(&remote.fs)?;
        let entry = fs.stat(&remote.path).await?;
        if entry.is_dir() {
            return Err(Error::Invalid(format!("not a file: {}", remote.path)));
        }
        let id = Uuid::new_v4();
        let local = self.inner.root.join(id.to_string()).join(&entry.name);
        download(&*fs, &remote.path, &local).await?;

        let info = EditSession {
            id,
            remote,
            local_path: local.to_string_lossy().into_owned(),
            status: EditStatus::Watching,
            uploads: 0,
            last_upload_ms: None,
            error: None,
        };
        let cancel = CancellationToken::new();
        self.inner.sessions.lock().unwrap().insert(
            id,
            Running {
                info: info.clone(),
                cancel: cancel.clone(),
            },
        );
        let _ = self.inner.tx.send(info.clone());
        let remote_path = info.remote.path.clone();
        tokio::spawn(watch(
            self.inner.clone(),
            id,
            fs,
            remote_path,
            local,
            cancel,
        ));
        Ok(info)
    }

    /// Stop watching and delete the local copy.
    pub fn stop(&self, id: Uuid) -> bool {
        let Some(r) = self.inner.sessions.lock().unwrap().remove(&id) else {
            return false;
        };
        r.cancel.cancel();
        let mut info = r.info;
        info.status = EditStatus::Stopped;
        let _ = self.inner.tx.send(info.clone());
        let dir = self.inner.root.join(id.to_string());
        let _ = std::fs::remove_dir_all(dir);
        true
    }

    /// Stop every session whose file lives on file system `fs` (on disconnect).
    pub fn stop_all_on(&self, fs: &str) {
        let ids: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.info.remote.fs == fs)
            .map(|r| r.info.id)
            .collect();
        for id in ids {
            self.stop(id);
        }
    }
}

fn update(inner: &Inner, id: Uuid, f: impl FnOnce(&mut EditSession)) {
    let snapshot = {
        let mut map = inner.sessions.lock().unwrap();
        let Some(r) = map.get_mut(&id) else { return };
        f(&mut r.info);
        r.info.clone()
    };
    let _ = inner.tx.send(snapshot);
}

async fn watch(
    inner: Arc<Inner>,
    id: Uuid,
    fs: Arc<dyn FileSystem>,
    remote_path: String,
    local: PathBuf,
    cancel: CancellationToken,
) {
    let mut last = signature(&local);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(POLL) => {}
        }
        let now = signature(&local);
        if now.is_none() || now == last {
            continue; // unchanged, or mid-save by an editor that replaces the file
        }
        // Wait until the editor has finished writing.
        tokio::time::sleep(POLL / 2).await;
        if signature(&local) != now {
            continue;
        }
        last = now;
        update(&inner, id, |s| s.status = EditStatus::Uploading);
        match upload(&*fs, &local, &remote_path).await {
            Ok(()) => update(&inner, id, |s| {
                s.status = EditStatus::Uploaded;
                s.uploads += 1;
                s.last_upload_ms = Some(crate::now_ms());
                s.error = None;
            }),
            Err(e) => update(&inner, id, |s| {
                s.status = EditStatus::Failed;
                s.error = Some(e.to_string());
            }),
        }
    }
}

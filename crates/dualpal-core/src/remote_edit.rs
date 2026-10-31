//! Editing remote files in a local app.
//!
//! The file is downloaded to a private folder and opened in the user's
//! editor (the front end does the opening). A watcher polls the local copy
//! and uploads it again every time it is saved, until the session is
//! stopped. Polling instead of OS file events keeps this identical on macOS
//! and Windows and copes with editors that save by replacing the file.
//!
//! Saves are never thrown away: a failed upload is retried, and each copy
//! keeps a small manifest next to it so that saves that had not reached the
//! server when the app closed (or the connection dropped) are offered again
//! on the next run instead of being deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::vfs::{safe_local_name, FileSystem, Location, Registry};

const POLL: Duration = Duration::from_millis(800);
/// Wait before retrying a failed upload.
const RETRY: Duration = Duration::from_secs(5);
const MANIFEST: &str = "session.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EditStatus {
    /// Waiting for the editor to save.
    Watching,
    Uploading,
    /// The last save was uploaded.
    Uploaded,
    /// The last save is not on the server yet (see `error`). It is retried
    /// while the server is connected, or with [`RemoteEditor::upload_now`].
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

/// (modified time in ms, size) of a local file.
type Signature = (i64, u64);

/// Kept next to each local copy.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Manifest {
    remote: Location,
    file: String,
    /// Signature of the local copy as last downloaded or uploaded.
    synced: Option<Signature>,
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

fn io(p: &Path, e: std::io::Error) -> Error {
    Error::io(p.to_string_lossy(), e)
}

/// Copy a file from any file system to a local path. The data goes to a
/// temporary sibling first, so `to` only ever holds a complete copy.
pub async fn download(fs: &dyn FileSystem, path: &str, to: &Path) -> Result<()> {
    let dir = to
        .parent()
        .ok_or_else(|| Error::Invalid(format!("no folder for {}", to.display())))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| io(dir, e))?;
    let part = dir.join(format!(
        ".{}.{}.part",
        to.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        Uuid::new_v4().simple()
    ));
    let result = async {
        let mut r = fs.open_read(path).await?;
        let mut w = tokio::fs::File::create(&part)
            .await
            .map_err(|e| io(&part, e))?;
        tokio::io::copy(&mut r, &mut w)
            .await
            .map_err(|e| Error::io(path, e))?;
        w.flush().await.map_err(|e| io(&part, e))?;
        drop(w);
        tokio::fs::rename(&part, to).await.map_err(|e| io(to, e))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

async fn upload(fs: &dyn FileSystem, from: &Path, path: &str) -> Result<()> {
    let mut r = tokio::fs::File::open(from).await.map_err(|e| io(from, e))?;
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).await.map_err(|e| io(from, e))?;
    let mut w = fs.open_write(path).await?;
    w.write_all(&buf).await.map_err(|e| Error::io(path, e))?;
    w.shutdown().await.map_err(|e| Error::io(path, e))
}

fn signature(p: &Path) -> Option<Signature> {
    let m = std::fs::metadata(p).ok()?;
    let ms = m
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_millis() as i64;
    Some((ms, m.len()))
}

fn write_manifest(dir: &Path, m: &Manifest) {
    if let Ok(json) = serde_json::to_vec_pretty(m) {
        let _ = std::fs::write(dir.join(MANIFEST), json);
    }
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).ok()?).ok()
}

impl RemoteEditor {
    /// `root` is a private folder for the downloaded copies. Copies left by
    /// an earlier run are cleaned up, except those holding saves that never
    /// reached the server: they come back as failed sessions.
    pub fn new(registry: Arc<Registry>, root: impl Into<PathBuf>) -> Self {
        let (tx, _) = broadcast::channel(64);
        let editor = Self {
            inner: Arc::new(Inner {
                registry,
                root: root.into(),
                sessions: Mutex::new(HashMap::new()),
                tx,
            }),
        };
        editor.recover();
        editor
    }

    fn recover(&self) {
        let Ok(dirs) = std::fs::read_dir(&self.inner.root) else {
            return;
        };
        for d in dirs.flatten() {
            let dir = d.path();
            let id = d.file_name().to_str().and_then(|n| Uuid::parse_str(n).ok());
            let manifest = read_manifest(&dir);
            let (Some(id), Some(m)) = (id, manifest) else {
                let _ = std::fs::remove_dir_all(&dir);
                continue;
            };
            let local = dir.join(&m.file);
            let now = signature(&local);
            if now.is_none() || now == m.synced {
                let _ = std::fs::remove_dir_all(&dir); // nothing unsent
                continue;
            }
            let info = EditSession {
                id,
                remote: m.remote,
                local_path: local.to_string_lossy().into_owned(),
                status: EditStatus::Failed,
                uploads: 0,
                last_upload_ms: None,
                error: Some("Dual Pal closed before this save was uploaded".into()),
            };
            self.inner.sessions.lock().unwrap().insert(
                id,
                Running {
                    info,
                    cancel: CancellationToken::new(),
                },
            );
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
        let existing = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .find(|r| r.info.remote == remote)
            .map(|r| r.info.clone());
        if let Some(s) = existing {
            if s.status == EditStatus::Failed {
                // Unsent changes: don't download over them, send them.
                return self.upload_now(s.id).await;
            }
            return Ok(s);
        }
        let fs = self.inner.registry.get(&remote.fs)?;
        let entry = fs.stat(&remote.path).await?;
        if entry.is_dir() {
            return Err(Error::Invalid(format!("not a file: {}", remote.path)));
        }
        let id = Uuid::new_v4();
        let dir = self.inner.root.join(id.to_string());
        // Server names can hold characters that mean something else here.
        let file = safe_local_name(&entry.name);
        let local = dir.join(&file);
        download(&*fs, &remote.path, &local).await?;
        write_manifest(
            &dir,
            &Manifest {
                remote: remote.clone(),
                file,
                synced: signature(&local),
            },
        );

        let info = EditSession {
            id,
            remote,
            local_path: local.to_string_lossy().into_owned(),
            status: EditStatus::Watching,
            uploads: 0,
            last_upload_ms: None,
            error: None,
        };
        self.spawn_watcher(info.clone(), fs, signature(&local));
        Ok(info)
    }

    fn spawn_watcher(&self, info: EditSession, fs: Arc<dyn FileSystem>, synced: Option<Signature>) {
        let cancel = CancellationToken::new();
        let id = info.id;
        let old = self.inner.sessions.lock().unwrap().insert(
            id,
            Running {
                info: info.clone(),
                cancel: cancel.clone(),
            },
        );
        if let Some(old) = old {
            old.cancel.cancel();
        }
        let _ = self.inner.tx.send(info.clone());
        tokio::spawn(watch(
            self.inner.clone(),
            id,
            fs,
            info.remote.path.clone(),
            PathBuf::from(info.local_path),
            synced,
            cancel,
        ));
    }

    /// Upload the local copy now and (re)start watching it, e.g. for a save
    /// that failed or was left over from an earlier run.
    pub async fn upload_now(&self, id: Uuid) -> Result<EditSession> {
        let info = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .get(&id)
            .map(|r| r.info.clone())
            .ok_or_else(|| Error::NotFound(format!("edit session {id}")))?;
        let fs = self.inner.registry.get(&info.remote.fs)?;
        let local = PathBuf::from(&info.local_path);
        update(&self.inner, id, |s| s.status = EditStatus::Uploading);
        let sig = signature(&local);
        match upload(&*fs, &local, &info.remote.path).await {
            Ok(()) => {
                mark_synced(&local, sig);
                let mut info = info;
                info.status = EditStatus::Uploaded;
                info.uploads += 1;
                info.last_upload_ms = Some(crate::now_ms());
                info.error = None;
                self.spawn_watcher(info.clone(), fs, sig);
                Ok(info)
            }
            Err(e) => {
                update(&self.inner, id, |s| {
                    s.status = EditStatus::Failed;
                    s.error = Some(e.to_string());
                });
                Err(e)
            }
        }
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

    /// The server `fs` was disconnected: end its sessions. Copies with saves
    /// that are not on the server yet are kept as failed sessions.
    pub fn stop_all_on(&self, fs: &str) {
        let sessions: Vec<_> = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.info.remote.fs == fs)
            .map(|r| r.info.clone())
            .collect();
        for s in sessions {
            let dir = self.inner.root.join(s.id.to_string());
            let synced = read_manifest(&dir).and_then(|m| m.synced);
            if signature(Path::new(&s.local_path)) == synced {
                self.stop(s.id);
                continue;
            }
            if let Some(r) = self.inner.sessions.lock().unwrap().get(&s.id) {
                r.cancel.cancel();
            }
            update(&self.inner, s.id, |s| {
                s.status = EditStatus::Failed;
                s.error = Some("disconnected before this save was uploaded".into());
            });
        }
    }
}

/// Record that the copy at `local` (with signature `sig`) is on the server.
fn mark_synced(local: &Path, sig: Option<Signature>) {
    let Some(dir) = local.parent() else { return };
    if let Some(mut m) = read_manifest(dir) {
        m.synced = sig;
        write_manifest(dir, &m);
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
    mut synced: Option<Signature>,
    cancel: CancellationToken,
) {
    let mut delay = POLL;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        delay = POLL;
        let now = signature(&local);
        if now.is_none() || now == synced {
            continue; // unchanged, or mid-save by an editor that replaces the file
        }
        // Wait until the editor has finished writing.
        tokio::time::sleep(POLL / 2).await;
        if signature(&local) != now {
            continue;
        }
        update(&inner, id, |s| s.status = EditStatus::Uploading);
        match upload(&*fs, &local, &remote_path).await {
            Ok(()) => {
                // Only a save that reached the server counts as synced.
                synced = now;
                mark_synced(&local, now);
                update(&inner, id, |s| {
                    s.status = EditStatus::Uploaded;
                    s.uploads += 1;
                    s.last_upload_ms = Some(crate::now_ms());
                    s.error = None;
                });
            }
            Err(e) => {
                update(&inner, id, |s| {
                    s.status = EditStatus::Failed;
                    s.error = Some(e.to_string());
                });
                delay = RETRY;
            }
        }
    }
}

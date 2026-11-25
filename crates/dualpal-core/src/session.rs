//! The window's last session (open tabs, their folders, pane layout),
//! persisted as JSON so the next launch can pick up where the last one ended.
//!
//! The UI owns the shape of the session and checks it when it reads it back,
//! so this store keeps it as an opaque JSON value. It lives in its own file,
//! apart from [`crate::settings`], because it is saved often and must never
//! overwrite a preference saved at the same moment (or be overwritten by one).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

use crate::error::{Error, Result};

/// Larger files are not a session this app wrote: they are ignored.
const MAX_BYTES: u64 = 1024 * 1024;

pub struct SessionStore {
    path: Option<PathBuf>,
    current: Mutex<Option<Value>>,
}

impl SessionStore {
    /// Read the session saved at `path`. A missing, unreadable, oversized or
    /// corrupt file gives no session, so the app starts with its defaults.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let current = std::fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= MAX_BYTES)
            .and_then(|_| std::fs::read(&path).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(Value::is_object);
        Self {
            path: Some(path),
            current: Mutex::new(current),
        }
    }

    /// A store that is never written to disk.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            current: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Option<Value> {
        self.current.lock().unwrap().clone()
    }

    /// Replace the saved session. The file is written under a temporary name
    /// first and then renamed, so a crash mid-write leaves the old session.
    pub fn set(&self, session: Value) -> Result<()> {
        if !session.is_object() {
            return Err(Error::Invalid("a session must be a JSON object".into()));
        }
        let json =
            serde_json::to_vec_pretty(&session).map_err(|e| Error::Invalid(e.to_string()))?;
        if json.len() as u64 > MAX_BYTES {
            return Err(Error::Invalid("the session is too large to save".into()));
        }
        // Hold the lock while writing so two saves cannot interleave.
        let mut current = self.current.lock().unwrap();
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
            }
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json).map_err(|e| Error::io(tmp.to_string_lossy(), e))?;
            std::fs::rename(&tmp, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::io(path.to_string_lossy(), e)
            })?;
        }
        *current = Some(session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_file_gives_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open(dir.path().join("session.json"));
        assert_eq!(store.get(), None);
    }

    #[test]
    fn round_trip_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("session.json");
        let session = json!({ "version": 1, "split": 0.4, "sides": [] });
        SessionStore::open(&path).set(session.clone()).unwrap();
        assert_eq!(SessionStore::open(&path).get(), Some(session));
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn corrupt_or_wrong_shaped_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        for bad in ["{ not json", "[1, 2]", "\"text\"", ""] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(SessionStore::open(&path).get(), None, "{bad:?}");
        }
    }

    #[test]
    fn oversized_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let big = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_BYTES as usize));
        std::fs::write(&path, big).unwrap();
        assert_eq!(SessionStore::open(&path).get(), None);
    }

    #[test]
    fn rejects_non_objects() {
        let store = SessionStore::in_memory();
        assert!(store.set(json!([1])).is_err());
        assert!(store.set(json!(null)).is_err());
        assert_eq!(store.get(), None);
    }

    #[test]
    fn corrupt_file_is_replaced_by_the_next_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(&path, "garbage").unwrap();
        let store = SessionStore::open(&path);
        store.set(json!({ "version": 1 })).unwrap();
        assert_eq!(
            SessionStore::open(&path).get(),
            Some(json!({ "version": 1 }))
        );
    }
}

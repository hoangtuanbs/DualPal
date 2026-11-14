//! Small user preferences, persisted as JSON.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// App used to edit files ("Edit in…"), e.g. `/Applications/Visual Studio
    /// Code.app` or `C:\Program Files\Notepad++\notepad++.exe`. Empty means
    /// the system's default app for the file type.
    pub editor: Option<String>,
    /// Command line of the app used to compare two files, with `{left}`
    /// and `{right}` (see [`crate::difftools`]). Empty means the first one found.
    pub diff_tool: Option<String>,
    /// When the app last looked for an update, in milliseconds since the Unix
    /// epoch. The UI checks at most once a day.
    pub last_update_check: Option<i64>,
    /// `Some(false)` turns off the automatic daily check for a new release.
    /// Empty means on. A check from the About window still works.
    pub update_checks: Option<bool>,
    /// A release the person chose to skip, e.g. `0.3.0`. The automatic check
    /// does not announce it again; a newer release is announced as usual.
    pub skipped_version: Option<String>,
}

pub struct SettingsStore {
    path: Option<PathBuf>,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let current = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    pub fn set(&self, s: Settings) -> Result<()> {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
            }
            let json = serde_json::to_vec_pretty(&s).map_err(|e| Error::Invalid(e.to_string()))?;
            std::fs::write(path, json).map_err(|e| Error::io(path.to_string_lossy(), e))?;
        }
        *self.current.lock().unwrap() = s;
        Ok(())
    }
}

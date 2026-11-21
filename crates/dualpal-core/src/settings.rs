//! Small user preferences, persisted as JSON.

use std::collections::BTreeMap;
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

    // ---- Settings screen ----
    /// Light or dark colours, or follow the system.
    pub theme: Theme,
    /// Text size of the window (file lists, sidebar, dialogs).
    pub font_size: FontSize,
    /// Height of the rows in the file lists.
    pub density: Density,
    /// New panes and tabs show hidden files.
    pub show_hidden: bool,
    /// List view columns that are turned off, e.g. `size` or `modified`.
    /// Listing what is hidden, not what is shown, keeps columns added later
    /// visible for people with an older file.
    pub hidden_columns: Vec<String>,
    /// What each pane (left, right) opens at start. Missing entries mean
    /// the home folder, as before this setting existed.
    pub pane_start: Vec<PaneStart>,
    /// The local folder each pane (left, right) last showed, for
    /// [`StartMode::Last`].
    pub last_folders: Vec<Option<String>>,
    /// Keyboard shortcuts changed from the defaults: command id → key
    /// combinations such as `Mod+Shift+P` or `F5`. An empty list unbinds the
    /// command. Commands not listed keep their default keys, which live in
    /// the UI.
    pub key_bindings: BTreeMap<String, Vec<String>>,
}

/// Declares a string-valued setting: written in lowercase, and a value this
/// version does not know (from a newer one) reads as the default instead of
/// making the whole file unreadable.
macro_rules! choice {
    ($(#[$meta:meta])* $name:ident { $default:ident = $default_s:literal $(, $variant:ident = $s:literal)* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase", from = "String")]
        pub enum $name {
            #[default]
            $default,
            $($variant,)*
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                match s.as_str() {
                    $($s => Self::$variant,)*
                    _ => Self::$default,
                }
            }
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    Self::$default => $default_s,
                    $(Self::$variant => $s,)*
                }
            }
        }
    };
}

choice!(
    /// Colour scheme of the window.
    Theme { System = "system", Light = "light", Dark = "dark" }
);
choice!(
    /// Text size of the window.
    FontSize { Normal = "normal", Small = "small", Large = "large" }
);
choice!(
    /// Row height of the file lists.
    Density { Normal = "normal", Compact = "compact" }
);
choice!(
    /// What a pane shows when the app starts.
    StartMode { Home = "home", Last = "last", Folder = "folder" }
);

/// What one pane opens at start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PaneStart {
    pub mode: StartMode,
    /// The folder for [`StartMode::Folder`].
    pub folder: Option<String>,
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
        let mut current = self.current.lock().unwrap();
        self.write(&s)?;
        *current = s;
        Ok(())
    }

    /// Change only the fields named in `patch` (a JSON object in the file's
    /// camelCase shape) and return the result. Reading and writing happen
    /// under one lock, so two windows or two parts of the UI changing
    /// different settings do not undo each other.
    pub fn update(&self, patch: serde_json::Value) -> Result<Settings> {
        let serde_json::Value::Object(patch) = patch else {
            return Err(Error::Invalid("settings change must be an object".into()));
        };
        let mut current = self.current.lock().unwrap();
        let mut value =
            serde_json::to_value(&*current).map_err(|e| Error::Invalid(e.to_string()))?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.extend(patch);
        }
        let next: Settings =
            serde_json::from_value(value).map_err(|e| Error::Invalid(e.to_string()))?;
        self.write(&next)?;
        *current = next.clone();
        Ok(next)
    }

    fn write(&self, s: &Settings) -> Result<()> {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
            }
            let json = serde_json::to_vec_pretty(s).map_err(|e| Error::Invalid(e.to_string()))?;
            std::fs::write(path, json).map_err(|e| Error::io(path.to_string_lossy(), e))?;
        }
        Ok(())
    }
}

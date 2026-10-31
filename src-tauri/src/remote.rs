//! Commands for remote connections, remote editing and settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dualpal_core::connections::{self, ConnectOptions, Connection, RemoteFavorite, SecretStore};
use dualpal_core::remote_edit::{self, EditSession};
use dualpal_core::settings::Settings;
use dualpal_core::vfs::{LocalFs, SshConfigHost};
use dualpal_core::{Error, Location};
use serde::Serialize;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

/// Secrets in the macOS Keychain / Windows Credential Manager. When the
/// system store is unavailable (or refuses), secrets are kept in memory for
/// this run only, so connecting still works.
#[derive(Default)]
pub struct KeychainSecrets {
    fallback: Mutex<HashMap<String, String>>,
}

const SERVICE: &str = "Dual Pal";

impl SecretStore for KeychainSecrets {
    fn get(&self, key: &str) -> CmdResult<Option<String>> {
        if let Some(s) = self.fallback.lock().unwrap().get(key) {
            return Ok(Some(s.clone()));
        }
        Ok(keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.get_password())
            .ok())
    }

    fn set(&self, key: &str, secret: &str) -> CmdResult<()> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.set_password(secret)) {
            Ok(()) => {
                self.fallback.lock().unwrap().remove(key);
            }
            Err(e) => {
                eprintln!("keychain unavailable ({e}); keeping the secret for this session only");
                self.fallback
                    .lock()
                    .unwrap()
                    .insert(key.into(), secret.into());
            }
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> CmdResult<()> {
        self.fallback.lock().unwrap().remove(key);
        let _ = keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential());
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    #[serde(flatten)]
    connection: Connection,
    connected: bool,
    fs_id: String,
}

fn view(state: &AppState, c: Connection) -> ConnectionView {
    let fs_id = c.fs_id();
    ConnectionView {
        connected: state.registry.get(&fs_id).is_ok(),
        fs_id,
        connection: c,
    }
}

#[tauri::command]
pub fn connections(state: State<'_, AppState>) -> Vec<ConnectionView> {
    state
        .connections
        .list()
        .into_iter()
        .map(|c| view(&state, c))
        .collect()
}

/// Add or update a connection. `secret`: `None` keeps the stored one, an
/// empty string removes it.
#[tauri::command]
pub fn save_connection(
    state: State<'_, AppState>,
    connection: Connection,
    secret: Option<String>,
) -> CmdResult<ConnectionView> {
    let c = state.connections.save(connection, secret)?;
    Ok(view(&state, c))
}

#[tauri::command]
pub async fn remove_connection(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    disconnect_fs(&state, &connections::fs_id(&id)).await;
    state.connections.remove(&id)
}

#[tauri::command]
pub fn reorder_connections(state: State<'_, AppState>, ids: Vec<String>) -> CmdResult<()> {
    state.connections.reorder(&ids)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    location: Location,
    name: String,
}

/// Connect, register the file system and return the folder to open.
/// `secret` is a password typed into a prompt; `remember` stores it.
/// `trust` is a host key fingerprint the user has just accepted.
#[tauri::command]
pub async fn connect(
    state: State<'_, AppState>,
    id: String,
    secret: Option<String>,
    remember: bool,
    trust: Option<String>,
) -> CmdResult<Connected> {
    let conn = state.connections.get(&id)?;
    let fs_id = conn.fs_id();
    if state.registry.get(&fs_id).is_ok() {
        // Already connected: open its start folder again.
        let start = state.start_paths.lock().unwrap().get(&fs_id).cloned();
        return Ok(Connected {
            location: Location::new(fs_id, start.unwrap_or_else(|| "/".into())),
            name: conn.name,
        });
    }
    let stored = if conn.uses_secret() {
        state.connections.secret(&id).unwrap_or(None)
    } else {
        None
    };
    let c = connections::connect(
        &conn,
        stored,
        ConnectOptions {
            secret: secret.clone(),
            trust_fingerprint: trust,
            known_hosts: None,
        },
    )
    .await?;
    if remember {
        if let Some(s) = secret.filter(|s| !s.is_empty()) {
            let _ = state.connections.save(conn.clone(), Some(s));
        }
    }
    state.registry.register(c.fs.clone());
    state
        .start_paths
        .lock()
        .unwrap()
        .insert(fs_id.clone(), c.start_path.clone());
    Ok(Connected {
        location: Location::new(fs_id, c.start_path),
        name: conn.name,
    })
}

async fn disconnect_fs(state: &AppState, fs_id: &str) {
    state.editor.stop_all_on(fs_id);
    // The session closes once running operations drop their handle to it.
    state.registry.unregister(fs_id);
    state.start_paths.lock().unwrap().remove(fs_id);
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    disconnect_fs(&state, &connections::fs_id(&id)).await;
    Ok(())
}

#[tauri::command]
pub fn add_connection_favorite(
    state: State<'_, AppState>,
    id: String,
    favorite: RemoteFavorite,
) -> CmdResult<ConnectionView> {
    let c = state.connections.add_favorite(&id, favorite)?;
    Ok(view(&state, c))
}

#[tauri::command]
pub fn remove_connection_favorite(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> CmdResult<ConnectionView> {
    let c = state.connections.remove_favorite(&id, &path)?;
    Ok(view(&state, c))
}

#[tauri::command]
pub fn ssh_config_hosts() -> Vec<SshConfigHost> {
    dualpal_core::vfs::ssh_config_hosts(None)
}

// ---- editing and previewing remote files -----------------------------------

fn open_with(app: &AppHandle, path: &str, with: Option<&str>) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(path, with).map_err(|e| {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        let app = with.map(|w| format!(" with {w}")).unwrap_or_default();
        Error::Unsupported(format!("opening {name}{app} ({e})"))
    })
}

/// Open a file in an app. Remote files are downloaded first and uploaded
/// again whenever the app saves them. `editor` true uses the preferred
/// editor from Settings instead of the default app.
#[tauri::command]
pub async fn open_file(
    app: AppHandle,
    state: State<'_, AppState>,
    location: Location,
    editor: bool,
) -> CmdResult<Option<EditSession>> {
    let with = if editor {
        state.settings.get().editor.filter(|e| !e.trim().is_empty())
    } else {
        None
    };
    if location.fs == LocalFs::ID {
        open_with(&app, &location.path, with.as_deref())?;
        return Ok(None);
    }
    let session = state.editor.start(location).await?;
    if let Err(e) = open_with(&app, &session.local_path, with.as_deref()) {
        // Nothing will edit the copy: don't leave it watched.
        state.editor.stop(session.id);
        return Err(e);
    }
    Ok(Some(session))
}

#[tauri::command]
pub fn edit_sessions(state: State<'_, AppState>) -> Vec<EditSession> {
    state.editor.list()
}

#[tauri::command]
pub fn stop_edit_session(state: State<'_, AppState>, id: Uuid) -> bool {
    state.editor.stop(id)
}

/// Largest remote file downloaded for the preview panel.
const PREVIEW_LIMIT: u64 = 256 * 1024 * 1024;

/// A local path for previewing a file: the file itself when it is local,
/// otherwise a cached download (refreshed when the remote file changes).
#[tauri::command]
pub async fn preview_path(state: State<'_, AppState>, location: Location) -> CmdResult<String> {
    if location.fs == LocalFs::ID {
        return Ok(location.path);
    }
    let fs = state.registry.get(&location.fs)?;
    let entry = fs.stat(&location.path).await?;
    if entry.size > PREVIEW_LIMIT {
        return Err(Error::Unsupported(
            "previewing a remote file this large".into(),
        ));
    }
    // Key the cache on location, size and date so a changed file is fetched again.
    let key = format!(
        "{:x}",
        fxhash(&format!(
            "{}|{}|{}|{:?}",
            location.fs, location.path, entry.size, entry.modified_ms
        ))
    );
    let local = state.preview_dir.join(key).join(&entry.name);
    if !local.exists() {
        remote_edit::download(fs.as_ref(), &location.path, &local).await?;
    }
    Ok(local.to_string_lossy().into_owned())
}

/// FNV-1a: a stable, dependency-free hash for cache folder names.
fn fxhash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

// ---- settings ----------------------------------------------------------------

#[tauri::command]
pub fn settings(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<()> {
    state.settings.set(settings)
}

pub fn secrets() -> Arc<dyn SecretStore> {
    Arc::new(KeychainSecrets::default())
}

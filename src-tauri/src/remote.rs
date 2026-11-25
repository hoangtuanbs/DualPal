//! Commands for remote connections, remote editing and settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dualpal_core::connections::{self, ConnectOptions, Connection, RemoteFavorite, SecretStore};
use dualpal_core::import::{self, ImportReport};
use dualpal_core::remote_edit::{self, EditSession};
use dualpal_core::settings::Settings;
use dualpal_core::vault::VaultStatus;
use dualpal_core::vfs::{LocalFs, SshConfigHost};
use dualpal_core::{Error, Location};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

/// Secrets in the macOS Keychain, Windows Credential Manager or the Linux
/// Secret Service (GNOME Keyring, KWallet). When the system store is
/// unavailable (or refuses), secrets are kept in memory for this run only, so
/// connecting still works.
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

    fn get_strict(&self, key: &str) -> CmdResult<Option<String>> {
        if let Some(s) = self.fallback.lock().unwrap().get(key) {
            return Ok(Some(s.clone()));
        }
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.get_password()) {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error("reading", e)),
        }
    }

    fn set_durable(&self, key: &str, secret: &str) -> CmdResult<()> {
        keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.set_password(secret))
            .map_err(|e| keyring_error("saving to", e))?;
        self.fallback.lock().unwrap().remove(key);
        Ok(())
    }

    fn delete_strict(&self, key: &str) -> CmdResult<()> {
        self.fallback.lock().unwrap().remove(key);
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error("removing a password from", e)),
        }
    }
}

fn keyring_error(what: &str, e: keyring::Error) -> Error {
    Error::Unsupported(format!("{what} the system keyring ({e})"))
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

/// Read the servers in another app's export (`source`: "winscp", "filezilla"
/// or "cyberduck"). Nothing is saved here: the UI shows the report and saves
/// the selected servers with `save_connection`.
#[tauri::command]
pub fn import_connections(source: String, text: String) -> CmdResult<ImportReport> {
    match source.as_str() {
        "winscp" => Ok(import::parse_winscp_ini(&text)),
        "filezilla" => Ok(import::parse_filezilla_sitemanager(&text)),
        "cyberduck" => Ok(import::parse_cyberduck_bookmarks(&text)),
        other => Err(Error::Invalid(format!("unknown import source {other}"))),
    }
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
        match state.connections.secret(&id) {
            // Ask for the master password, unless a password was just typed.
            Err(Error::Locked) if secret.is_none() => return Err(Error::Locked),
            r => r.unwrap_or(None),
        }
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
            known_certs: None,
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

fn open_with<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    path: &str,
    with: Option<&str>,
) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    if dualpal_core::launch_log::record("DUALPAL_OPENER_LOG", path) {
        return Ok(());
    }
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
pub async fn open_file<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
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

/// Upload a save that failed (or was left from an earlier run) now.
#[tauri::command]
pub async fn upload_edit_session(state: State<'_, AppState>, id: Uuid) -> CmdResult<EditSession> {
    state.editor.upload_now(id).await
}

#[tauri::command]
pub fn stop_edit_session(state: State<'_, AppState>, id: Uuid) -> bool {
    state.editor.stop(id)
}

/// Largest remote file downloaded for the preview panel.
const PREVIEW_LIMIT: u64 = 256 * 1024 * 1024;

/// A local path for a file: the file itself when it is local, otherwise a
/// cached download (fetched again when the remote file changes).
pub async fn local_copy(state: &AppState, location: Location, limit: u64) -> CmdResult<String> {
    if location.fs == LocalFs::ID {
        return Ok(location.path);
    }
    let fs = state.registry.get(&location.fs)?;
    let entry = fs.stat(&location.path).await?;
    if entry.is_dir() {
        return Err(Error::Unsupported("downloading a folder for this".into()));
    }
    if entry.size > limit {
        return Err(Error::Unsupported(
            "downloading a remote file this large for this".into(),
        ));
    }
    // Key the cache on location, size and date so a changed file is fetched again.
    let key = format!(
        "{:x}",
        dualpal_core::sync::fnv(&format!(
            "{}|{}|{}|{:?}",
            location.fs, location.path, entry.size, entry.modified_ms
        ))
    );
    // `download` writes a temporary file first, so an existing copy is complete.
    let local = state
        .preview_dir
        .join(key)
        .join(dualpal_core::vfs::safe_local_name(&entry.name));
    if !local.exists() {
        remote_edit::download(fs.as_ref(), &location.path, &local).await?;
    }
    Ok(local.to_string_lossy().into_owned())
}

/// A local path for previewing a file (see [`local_copy`]).
#[tauri::command]
pub async fn preview_path(state: State<'_, AppState>, location: Location) -> CmdResult<String> {
    local_copy(&state, location, PREVIEW_LIMIT).await
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

/// The last session's tabs and layout, or null when there is none to restore.
#[tauri::command]
pub fn session(state: State<'_, AppState>) -> Option<serde_json::Value> {
    state.session.get()
}

#[tauri::command]
pub fn save_session(state: State<'_, AppState>, session: serde_json::Value) -> CmdResult<()> {
    state.session.set(session)
}

// ---- master password ----------------------------------------------------------

#[tauri::command]
pub fn secrets_status(state: State<'_, AppState>) -> VaultStatus {
    state.vault.status()
}

/// Set a master password and move the saved secrets under it.
#[tauri::command]
pub fn protect_secrets(state: State<'_, AppState>, password: String) -> CmdResult<VaultStatus> {
    state.vault.enable(&password, &state.connections.ids())?;
    Ok(state.vault.status())
}

#[tauri::command]
pub fn unlock_secrets(state: State<'_, AppState>, password: String) -> CmdResult<VaultStatus> {
    state.vault.unlock(&password)?;
    // Servers removed while locked (in any run) leave their secret behind.
    state.vault.retain(&state.connections.ids())?;
    Ok(state.vault.status())
}

/// Forget the master password until it is entered again. Live connections
/// stay connected.
#[tauri::command]
pub fn lock_secrets(state: State<'_, AppState>) -> VaultStatus {
    state.vault.lock();
    state.vault.status()
}

#[tauri::command]
pub fn change_master_password(
    state: State<'_, AppState>,
    current: String,
    password: String,
) -> CmdResult<VaultStatus> {
    state.vault.unlock(&current)?;
    state.vault.retain(&state.connections.ids())?;
    state.vault.change(&current, &password)?;
    Ok(state.vault.status())
}

/// Stop using a master password: the secrets go back to the system store.
#[tauri::command]
pub fn unprotect_secrets(state: State<'_, AppState>, password: String) -> CmdResult<VaultStatus> {
    state.vault.unlock(&password)?;
    // Only the secrets of saved servers go back to the system store.
    state.vault.retain(&state.connections.ids())?;
    state.vault.disable(&password)?;
    Ok(state.vault.status())
}

/// Forgotten master password: delete the protected secrets. Saved servers stay.
#[tauri::command]
pub fn reset_secrets(state: State<'_, AppState>) -> CmdResult<VaultStatus> {
    state.vault.reset()?;
    Ok(state.vault.status())
}

pub fn secrets() -> Arc<dyn SecretStore> {
    Arc::new(KeychainSecrets::default())
}

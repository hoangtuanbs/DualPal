use dualpal_core::places::Favorite;
use dualpal_core::{ActivityRecord, Entry, Error, Location, Operation, OperationInfo};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemInfo {
    id: String,
    name: String,
}

#[tauri::command]
pub fn file_systems(state: State<'_, AppState>) -> Vec<FileSystemInfo> {
    state
        .registry
        .ids()
        .into_iter()
        .filter_map(|id| state.registry.get(&id).ok())
        .map(|fs| FileSystemInfo {
            id: fs.id().to_string(),
            name: fs.display_name(),
        })
        .collect()
}

/// The user's home folder on the local disk.
#[tauri::command]
pub fn home_location(app: tauri::AppHandle) -> CmdResult<Location> {
    use tauri::Manager;
    let home = app
        .path()
        .home_dir()
        .map_err(|e| Error::NotFound(e.to_string()))?;
    Ok(Location::new(
        dualpal_core::vfs::LocalFs::ID,
        home.to_string_lossy(),
    ))
}

#[tauri::command]
pub async fn list_dir(state: State<'_, AppState>, location: Location) -> CmdResult<Vec<Entry>> {
    let fs = state.registry.get(&location.fs)?;
    fs.list(&location.path).await
}

#[tauri::command]
pub fn parent_location(
    state: State<'_, AppState>,
    location: Location,
) -> CmdResult<Option<Location>> {
    let fs = state.registry.get(&location.fs)?;
    Ok(fs
        .parent(&location.path)
        .map(|p| Location::new(location.fs, p)))
}

/// Queue an operation; progress arrives as `operation-updated` events.
/// Async so that it runs on the Tokio runtime the queue spawns onto.
#[tauri::command]
pub async fn enqueue_operation(
    state: State<'_, AppState>,
    operation: Operation,
) -> CmdResult<Uuid> {
    Ok(state.queue.enqueue(operation))
}

#[tauri::command]
pub fn cancel_operation(state: State<'_, AppState>, id: Uuid) -> bool {
    state.queue.cancel(id)
}

#[tauri::command]
pub fn operations(state: State<'_, AppState>) -> Vec<OperationInfo> {
    state.queue.list()
}

#[tauri::command]
pub fn clear_finished_operations(state: State<'_, AppState>) {
    state.queue.clear_finished()
}

#[tauri::command]
pub fn activity(state: State<'_, AppState>) -> Vec<ActivityRecord> {
    state.queue.activity().list()
}

#[tauri::command]
pub fn clear_activity(state: State<'_, AppState>) -> CmdResult<()> {
    state.queue.activity().clear().map_err(|e| Error::Io {
        path: "activity log".into(),
        source: e,
    })
}

// ---- sidebar -----------------------------------------------------------------

#[tauri::command]
pub async fn volumes() -> Vec<dualpal_core::places::Volume> {
    tauri::async_runtime::spawn_blocking(dualpal_core::places::volumes)
        .await
        .unwrap_or_default()
}

/// Free bytes on the volume holding `location` (local only for now).
#[tauri::command]
pub async fn available_space(location: Location) -> Option<u64> {
    if location.fs != dualpal_core::vfs::LocalFs::ID {
        return None;
    }
    tauri::async_runtime::spawn_blocking(move || {
        dualpal_core::places::available_space(&location.path)
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub fn favorites(state: State<'_, AppState>) -> Vec<Favorite> {
    state.favorites.list()
}

#[tauri::command]
pub fn add_favorite(state: State<'_, AppState>, favorite: Favorite) -> CmdResult<Vec<Favorite>> {
    state
        .favorites
        .add(favorite)
        .map_err(|e| io_err("favorites", e))
}

#[tauri::command]
pub fn remove_favorite(state: State<'_, AppState>, location: Location) -> CmdResult<Vec<Favorite>> {
    state
        .favorites
        .remove(&location)
        .map_err(|e| io_err("favorites", e))
}

#[tauri::command]
pub fn reorder_favorites(
    state: State<'_, AppState>,
    from: usize,
    to: usize,
) -> CmdResult<Vec<Favorite>> {
    state
        .favorites
        .reorder(from, to)
        .map_err(|e| io_err("favorites", e))
}

/// Open a file with its default application.
#[tauri::command]
pub fn open_path(app: tauri::AppHandle, location: Location) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    if location.fs != dualpal_core::vfs::LocalFs::ID {
        return Err(Error::Unsupported("opening remote files".into()));
    }
    app.opener()
        .open_path(&location.path, None::<&str>)
        .map_err(|e| Error::Io {
            path: location.path.clone(),
            source: std::io::Error::other(e.to_string()),
        })
}

fn io_err(path: &str, e: std::io::Error) -> Error {
    Error::Io {
        path: path.into(),
        source: e,
    }
}

// ---- preview panel ---------------------------------------------------------

#[tauri::command]
pub async fn stat(state: State<'_, AppState>, location: Location) -> CmdResult<Entry> {
    state.registry.get(&location.fs)?.stat(&location.path).await
}

#[tauri::command]
pub async fn read_text(
    state: State<'_, AppState>,
    location: Location,
    max_bytes: usize,
) -> CmdResult<dualpal_core::vfs::TextFile> {
    let fs = state.registry.get(&location.fs)?;
    dualpal_core::vfs::read_text(fs.as_ref(), &location.path, max_bytes).await
}

#[tauri::command]
pub async fn write_text(
    state: State<'_, AppState>,
    location: Location,
    text: String,
) -> CmdResult<()> {
    let fs = state.registry.get(&location.fs)?;
    dualpal_core::vfs::write_text(fs.as_ref(), &location.path, &text).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutInfo {
    version: String,
    os: String,
    arch: String,
}

/// Version and platform for the About window and support emails.
#[tauri::command]
pub fn about_info(app: tauri::AppHandle) -> AboutInfo {
    AboutInfo {
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// Open a web or email link in the default browser or mail app.
/// Only https: and mailto: links are allowed.
#[tauri::command]
pub fn open_url(app: tauri::AppHandle, url: String) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    if !(url.starts_with("https://") || url.starts_with("mailto:")) {
        return Err(Error::Unsupported(format!("opening {url}")));
    }
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| Error::Io {
            path: url.clone(),
            source: std::io::Error::other(e.to_string()),
        })
}

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

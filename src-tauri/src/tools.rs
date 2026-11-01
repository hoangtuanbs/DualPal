//! Commands for the power tools: folder compare and sync, sync favorites,
//! Multi Rename, external compare apps and activity insights.

use std::collections::HashMap;
use std::sync::Arc;

use dualpal_core::activity::{self, Insights};
use dualpal_core::compare::CompareOptions;
use dualpal_core::difftools::{self, DiffTool};
use dualpal_core::rename::{self, RenameInput, RenamePreset, RenamePreview, RenameRule};
use dualpal_core::sync::{SyncAction, SyncFavorite, SyncMode, SyncPreview};
use dualpal_core::vfs::LocalFs;
use dualpal_core::{CancellationToken, Error, Location};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::AppState;

type CmdResult<T> = Result<T, Error>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompareProgress {
    folders: u64,
    current: String,
}

/// Compare two folders and plan a sync in `mode`. Progress is sent as
/// `compare-progress` events; [`cancel_compare`] stops it.
#[tauri::command]
pub async fn sync_plan(
    app: AppHandle,
    state: State<'_, AppState>,
    left: Location,
    right: Location,
    mode: SyncMode,
    options: CompareOptions,
) -> CmdResult<SyncPreview> {
    let cancel = CancellationToken::new();
    // A new comparison replaces one still running.
    if let Some(old) = state.compare_cancel.lock().unwrap().replace(cancel.clone()) {
        old.cancel();
    }
    let last = std::sync::Mutex::new(std::time::Instant::now());
    let progress: dualpal_core::compare::ProgressFn = Arc::new(move |folders, current: &str| {
        let mut last = last.lock().unwrap();
        if last.elapsed().as_millis() >= 100 {
            *last = std::time::Instant::now();
            let _ = app.emit(
                "compare-progress",
                CompareProgress {
                    folders,
                    current: current.to_string(),
                },
            );
        }
    });
    state
        .sync
        .plan(&left, &right, mode, options, cancel, Some(progress))
        .await
}

#[tauri::command]
pub fn cancel_compare(state: State<'_, AppState>) {
    if let Some(c) = state.compare_cancel.lock().unwrap().take() {
        c.cancel();
    }
}

/// Queue a planned sync, with the user's changed steps. Returns the operation id.
/// Async so that it runs on the Tokio runtime the queue spawns onto.
#[tauri::command]
pub async fn run_sync(
    state: State<'_, AppState>,
    plan: Uuid,
    overrides: HashMap<String, SyncAction>,
) -> CmdResult<Uuid> {
    Ok(state.sync.run(plan, &overrides)?.op)
}

/// Forget the last sync of a pair: the next one compares by date only.
#[tauri::command]
pub fn forget_sync_history(state: State<'_, AppState>, left: Location, right: Location) {
    state.sync.forget(&left, &right);
}

#[tauri::command]
pub fn sync_favorites(state: State<'_, AppState>) -> Vec<SyncFavorite> {
    state.sync_favorites.list()
}

#[tauri::command]
pub fn save_sync_favorite(
    state: State<'_, AppState>,
    favorite: SyncFavorite,
) -> CmdResult<SyncFavorite> {
    state.sync_favorites.save(favorite)
}

#[tauri::command]
pub fn remove_sync_favorite(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.sync_favorites.remove(&id)
}

// ---- Multi Rename ----------------------------------------------------------------

#[tauri::command]
pub fn rename_preview(
    rules: Vec<RenameRule>,
    inputs: Vec<RenameInput>,
    others: Vec<String>,
) -> CmdResult<Vec<RenamePreview>> {
    rename::preview(&rules, &inputs, &others)
}

#[tauri::command]
pub fn rename_presets(state: State<'_, AppState>) -> Vec<RenamePreset> {
    state.rename_presets.list()
}

#[tauri::command]
pub fn save_rename_preset(
    state: State<'_, AppState>,
    preset: RenamePreset,
) -> CmdResult<Vec<RenamePreset>> {
    state.rename_presets.save(preset)?;
    Ok(state.rename_presets.list())
}

#[tauri::command]
pub fn remove_rename_preset(
    state: State<'_, AppState>,
    name: String,
) -> CmdResult<Vec<RenamePreset>> {
    state.rename_presets.remove(&name)?;
    Ok(state.rename_presets.list())
}

// ---- external compare apps ----------------------------------------------------------

#[tauri::command]
pub async fn diff_tools() -> Vec<DiffTool> {
    tauri::async_runtime::spawn_blocking(difftools::detect)
        .await
        .unwrap_or_default()
}

/// Largest remote file downloaded to compare.
const COMPARE_LIMIT: u64 = 512 * 1024 * 1024;

/// Open two files (or two local folders) in the compare app from
/// Settings, or the first one installed. Remote files are downloaded first.
#[tauri::command]
pub async fn compare_files(
    state: State<'_, AppState>,
    left: Location,
    right: Location,
) -> CmdResult<()> {
    let template = match state
        .settings
        .get()
        .diff_tool
        .filter(|t| !t.trim().is_empty())
    {
        Some(t) => t,
        None => {
            let tools = tauri::async_runtime::spawn_blocking(difftools::detect)
                .await
                .unwrap_or_default();
            tools.into_iter().next().map(|t| t.command).ok_or_else(|| {
                Error::Unsupported(
                    "comparing files: no compare app found. Choose one in Compare Tool…".into(),
                )
            })?
        }
    };
    let mut paths = Vec::new();
    for loc in [left, right] {
        if loc.fs == LocalFs::ID {
            paths.push(loc.path);
        } else {
            paths.push(crate::remote::local_copy(&state, loc, COMPARE_LIMIT).await?);
        }
    }
    difftools::launch(&template, &paths[0], &paths[1])
}

// ---- activity insights ---------------------------------------------------------------

#[tauri::command]
pub fn activity_insights(
    state: State<'_, AppState>,
    days: u32,
    utc_offset_minutes: i32,
) -> Insights {
    activity::insights(
        &state.queue.activity().list(),
        dualpal_core::now_ms(),
        days,
        utc_offset_minutes,
    )
}

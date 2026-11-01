//! Tauri shell: wires `dualpal-core` to the web UI.
//!
//! Commands are thin wrappers around the core; operation progress is pushed
//! to the UI as `operation-updated` events.

mod commands;
mod remote;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dualpal_core::connections::Connections;
use dualpal_core::places::Favorites;
use dualpal_core::remote_edit::RemoteEditor;
use dualpal_core::rename::RenamePresets;
use dualpal_core::settings::SettingsStore;
use dualpal_core::sync::{SyncEngine, SyncFavorites};
use dualpal_core::{ActivityLog, CancellationToken, OperationQueue, Registry};
use tauri::{Emitter, Manager};

/// Keep this many finished operations in the activity log.
const ACTIVITY_CAPACITY: usize = 1000;
/// Operations allowed to run at the same time; the rest wait in the queue.
const MAX_CONCURRENT_OPS: usize = 2;

pub struct AppState {
    pub registry: Arc<Registry>,
    pub queue: OperationQueue,
    pub favorites: Favorites,
    pub connections: Connections,
    pub settings: SettingsStore,
    pub editor: RemoteEditor,
    /// Folder each live connection opened in, by file system id.
    pub start_paths: Mutex<HashMap<String, String>>,
    /// Downloaded copies of remote files for previews and compare apps.
    pub preview_dir: PathBuf,
    pub sync: Arc<SyncEngine>,
    pub sync_favorites: SyncFavorites,
    pub rename_presets: RenamePresets,
    /// Stops the folder comparison that is running, if any.
    pub compare_cancel: Mutex<Option<CancellationToken>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let registry = Arc::new(Registry::with_local());
            let data_dir = app.path().app_data_dir()?;
            let favorites = Favorites::open(data_dir.join("favorites.json"));
            let connections =
                Connections::open(data_dir.join("connections.json"), remote::secrets());
            let settings = SettingsStore::open(data_dir.join("settings.json"));
            let cache_dir = app.path().app_cache_dir()?;
            let preview_dir = cache_dir.join("preview");
            // Previews are only useful for one run of the app.
            let _ = std::fs::remove_dir_all(&preview_dir);
            // Kept across runs: copies holding saves that never reached the
            // server come back as sessions to upload.
            let edits_dir = data_dir.join("edits");
            let editor = RemoteEditor::new(registry.clone(), edits_dir);
            let log_path = data_dir.join("activity.jsonl");
            let activity = match ActivityLog::open(&log_path, ACTIVITY_CAPACITY) {
                Ok(log) => log,
                Err(e) => {
                    eprintln!(
                        "activity log unavailable ({}): {e}; using memory only",
                        log_path.display()
                    );
                    ActivityLog::in_memory(ACTIVITY_CAPACITY)
                }
            };
            let queue =
                OperationQueue::new(registry.clone(), Arc::new(activity), MAX_CONCURRENT_OPS);

            // Forward every progress snapshot to the UI.
            let mut rx = queue.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                loop {
                    match rx.recv().await {
                        Ok(info) => {
                            let _ = handle.emit("operation-updated", info);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            let mut edits = editor.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                loop {
                    match edits.recv().await {
                        Ok(s) => {
                            let _ = handle.emit("edit-session-updated", s);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            let sync = SyncEngine::new(queue.clone(), Some(data_dir.join("sync-state")));
            let sync_favorites = SyncFavorites::open(data_dir.join("syncs.json"));
            let rename_presets = RenamePresets::open(data_dir.join("rename-presets.json"));

            app.manage(AppState {
                registry,
                queue,
                favorites,
                connections,
                settings,
                editor,
                start_paths: Mutex::new(HashMap::new()),
                preview_dir,
                sync,
                sync_favorites,
                rename_presets,
                compare_cancel: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::file_systems,
            commands::home_location,
            commands::list_dir,
            commands::parent_location,
            commands::enqueue_operation,
            commands::cancel_operation,
            commands::operations,
            commands::clear_finished_operations,
            commands::activity,
            commands::clear_activity,
            commands::volumes,
            commands::available_space,
            commands::favorites,
            commands::add_favorite,
            commands::remove_favorite,
            commands::reorder_favorites,
            commands::open_path,
            commands::about_info,
            commands::open_url,
            commands::stat,
            commands::read_text,
            commands::write_text,
            remote::connections,
            remote::save_connection,
            remote::remove_connection,
            remote::reorder_connections,
            remote::connect,
            remote::disconnect,
            remote::add_connection_favorite,
            remote::remove_connection_favorite,
            remote::ssh_config_hosts,
            remote::open_file,
            remote::edit_sessions,
            remote::stop_edit_session,
            remote::upload_edit_session,
            remote::preview_path,
            remote::settings,
            remote::save_settings,
            tools::sync_plan,
            tools::cancel_compare,
            tools::run_sync,
            tools::forget_sync_history,
            tools::sync_favorites,
            tools::save_sync_favorite,
            tools::remove_sync_favorite,
            tools::rename_preview,
            tools::rename_presets,
            tools::save_rename_preset,
            tools::remove_rename_preset,
            tools::diff_tools,
            tools::compare_files,
            tools::activity_insights,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Dual Pal");
}

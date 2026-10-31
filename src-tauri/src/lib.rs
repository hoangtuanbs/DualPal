//! Tauri shell: wires `dualpal-core` to the web UI.
//!
//! Commands are thin wrappers around the core; operation progress is pushed
//! to the UI as `operation-updated` events.

mod commands;

use std::sync::Arc;

use dualpal_core::places::Favorites;
use dualpal_core::{ActivityLog, OperationQueue, Registry};
use tauri::{Emitter, Manager};

/// Keep this many finished operations in the activity log.
const ACTIVITY_CAPACITY: usize = 1000;
/// Operations allowed to run at the same time; the rest wait in the queue.
const MAX_CONCURRENT_OPS: usize = 2;

pub struct AppState {
    pub registry: Arc<Registry>,
    pub queue: OperationQueue,
    pub favorites: Favorites,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let registry = Arc::new(Registry::with_local());
            let data_dir = app.path().app_data_dir()?;
            let favorites = Favorites::open(data_dir.join("favorites.json"));
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

            app.manage(AppState {
                registry,
                queue,
                favorites,
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
            commands::stat,
            commands::read_text,
            commands::write_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Dual Pal");
}

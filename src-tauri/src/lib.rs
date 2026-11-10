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
use std::time::Duration;

use dualpal_core::connections::{Connections, MemorySecrets, SecretStore};
use dualpal_core::livesync::LiveSync;
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

/// Where the app keeps its state. Tests build this from a temp folder instead
/// of the real profile (PREP-01).
#[derive(Clone, Debug)]
pub struct AppDirs {
    pub data: PathBuf,
    pub cache: PathBuf,
}

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
    pub live: Arc<LiveSync>,
    pub sync_favorites: SyncFavorites,
    pub rename_presets: RenamePresets,
    /// Stops the folder comparison that is running, if any.
    pub compare_cancel: Mutex<Option<CancellationToken>>,
    /// Folder size calculations that are running, by request id.
    pub folder_size_cancel: Mutex<HashMap<u64, CancellationToken>>,
}

/// Build the whole application state on `dirs`. The shell calls this with the
/// real app folders; tests call it with a temp folder and [`MemorySecrets`].
pub fn build_state(dirs: AppDirs, secrets: Arc<dyn SecretStore>) -> std::io::Result<AppState> {
    let registry = Arc::new(Registry::with_local());
    let data_dir = dirs.data;
    let favorites = Favorites::open(data_dir.join("favorites.json"));
    let connections = Connections::open(data_dir.join("connections.json"), secrets);
    let settings = SettingsStore::open(data_dir.join("settings.json"));
    let cache_dir = dirs.cache;
    let preview_dir = cache_dir.join("preview");
    // Previews are only useful for one run of the app.
    let _ = std::fs::remove_dir_all(&preview_dir);
    // Kept across runs: copies holding saves that never reached the server
    // come back as sessions to upload.
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
    let queue = OperationQueue::new(registry.clone(), Arc::new(activity), MAX_CONCURRENT_OPS);
    let sync = SyncEngine::new(queue.clone(), Some(data_dir.join("sync-state")));
    let live = LiveSync::new(sync.clone(), queue.clone());
    let sync_favorites = SyncFavorites::open(data_dir.join("syncs.json"));
    let rename_presets = RenamePresets::open(data_dir.join("rename-presets.json"));

    Ok(AppState {
        registry,
        queue,
        favorites,
        connections,
        settings,
        editor,
        start_paths: Mutex::new(HashMap::new()),
        preview_dir,
        sync,
        live,
        sync_favorites,
        rename_presets,
        compare_cancel: Mutex::new(None),
        folder_size_cancel: Mutex::new(HashMap::new()),
    })
}

/// Forward every broadcast channel as a Tauri event. Called once after
/// [`build_state`].
pub fn spawn_event_forwarders<R: tauri::Runtime>(app: &tauri::AppHandle<R>, state: &AppState) {
    let mut rx = state.queue.subscribe();
    let handle = app.clone();
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

    let mut edits = state.editor.subscribe();
    let handle = app.clone();
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

    let mut live_rx = state.live.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match live_rx.recv().await {
                Ok(s) => {
                    let _ = handle.emit("live-sync-updated", s);
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Every command name registered with the IPC layer, for `CMD-000`.
pub const COMMANDS: &[&str] = &[
    "file_systems",
    "live_sync_start",
    "live_sync_stop",
    "live_sync_status",
    "home_location",
    "list_dir",
    "parent_location",
    "enqueue_operation",
    "cancel_operation",
    "operations",
    "clear_finished_operations",
    "activity",
    "clear_activity",
    "volumes",
    "available_space",
    "favorites",
    "add_favorite",
    "remove_favorite",
    "reorder_favorites",
    "open_path",
    "about_info",
    "open_url",
    "stat",
    "folder_size",
    "cancel_folder_size",
    "read_text",
    "write_text",
    "connections",
    "save_connection",
    "remove_connection",
    "reorder_connections",
    "connect",
    "disconnect",
    "add_connection_favorite",
    "remove_connection_favorite",
    "ssh_config_hosts",
    "open_file",
    "edit_sessions",
    "stop_edit_session",
    "upload_edit_session",
    "preview_path",
    "settings",
    "save_settings",
    "sync_plan",
    "cancel_compare",
    "run_sync",
    "forget_sync_history",
    "sync_favorites",
    "save_sync_favorite",
    "remove_sync_favorite",
    "rename_preview",
    "rename_presets",
    "save_rename_preset",
    "remove_rename_preset",
    "diff_tools",
    "compare_files",
    "activity_insights",
];

/// The single `generate_handler!` list, shared by the app and the tests.
macro_rules! all_commands {
    () => {
        tauri::generate_handler![
            commands::file_systems,
            tools::live_sync_start,
            tools::live_sync_stop,
            tools::live_sync_status,
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
            commands::folder_size,
            commands::cancel_folder_size,
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
        ]
    };
}

/// Register the same command list the app uses on a test builder (PREP-01).
pub fn register_handlers<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(all_commands!())
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn env_ms(var: &str) -> Option<u64> {
    std::env::var(var)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

/// App folders, honouring the test overrides (PREP-02).
fn app_dirs(app: &tauri::App) -> std::io::Result<AppDirs> {
    let mut active: Vec<&str> = Vec::new();
    let data = match env_path("DUALPAL_DATA_DIR") {
        Some(p) => {
            active.push("DUALPAL_DATA_DIR");
            p
        }
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| std::io::Error::other(e.to_string()))?,
    };
    let cache = match env_path("DUALPAL_CACHE_DIR") {
        Some(p) => {
            active.push("DUALPAL_CACHE_DIR");
            p
        }
        None => app
            .path()
            .app_cache_dir()
            .map_err(|e| std::io::Error::other(e.to_string()))?,
    };
    if std::env::var_os("DUALPAL_HOME").is_some() {
        active.push("DUALPAL_HOME");
    }
    if !active.is_empty() {
        eprintln!("dualpal: test overrides active: {}", active.join(", "));
    }
    Ok(AppDirs { data, cache })
}

fn secrets_from_env() -> Arc<dyn SecretStore> {
    if std::env::var("DUALPAL_SECRETS").as_deref() == Ok("memory") {
        eprintln!("dualpal: test overrides active: DUALPAL_SECRETS");
        Arc::new(MemorySecrets::default())
    } else {
        remote::secrets()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = build_state(app_dirs(app)?, secrets_from_env())?;
            if let Some(ms) = env_ms("DUALPAL_LIVE_POLL_MS") {
                state.live.with_poll_interval(Duration::from_millis(ms));
            }
            if let Some(ms) = env_ms("DUALPAL_WATCH_DEBOUNCE_MS") {
                state.live.with_watch_debounce(Duration::from_millis(ms));
            }
            spawn_event_forwarders(app.handle(), &state);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(all_commands!())
        .run(tauri::generate_context!())
        .expect("error while running Dual Pal");
}

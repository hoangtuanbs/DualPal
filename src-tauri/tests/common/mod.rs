//! Shared helpers for the Tauri command (L4) tests.

use std::sync::Arc;

use dualpal_core::connections::MemorySecrets;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::Manager;

pub type App = tauri::App<MockRuntime>;

/// Serializes tests that mutate process-wide environment variables.
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A running app on a temp folder, with an in-memory secret store.
pub struct TestApp {
    pub app: App,
    /// Kept alive so the data/cache folders exist for the whole test.
    pub tmp: tempfile::TempDir,
}

/// Build a real [`dualpal_app::AppState`] on temp folders and register the
/// exact command list the shipped app uses.
pub fn app() -> TestApp {
    let tmp = tempfile::tempdir().unwrap();
    let state = dualpal_app::build_state(
        dualpal_app::AppDirs {
            data: tmp.path().join("data"),
            cache: tmp.path().join("cache"),
        },
        Arc::new(MemorySecrets::default()),
    )
    .expect("build app state");
    let app = dualpal_app::register_handlers(mock_builder())
        .manage(state)
        .build(mock_context(noop_assets()))
        .expect("build app");
    let _ = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("build webview window");
    TestApp { app, tmp }
}

impl TestApp {
    /// Also start the operation / edit-session / live-sync event forwarders,
    /// so `operation-updated` and friends are emitted to the mock window.
    pub fn with_events(self) -> Self {
        let state = self.app.state::<dualpal_app::AppState>();
        dualpal_app::spawn_event_forwarders(self.app.handle(), &state);
        self
    }
}

/// Invoke `cmd` with a JSON object of arguments and deserialize the result.
/// A command error comes back as the serialized `{kind, message}` object.
pub fn call<T: serde::de::DeserializeOwned>(
    app: &App,
    cmd: &str,
    args: serde_json::Value,
) -> Result<T, serde_json::Value> {
    let webview = app.get_webview_window("main").expect("main window");
    let request = tauri::webview::InvokeRequest {
        cmd: cmd.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: if cfg!(any(windows, target_os = "android")) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .unwrap(),
        body: tauri::ipc::InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    };
    let response = tauri::test::get_ipc_response(&webview, request)?;
    response
        .deserialize::<T>()
        .map_err(|e| serde_json::json!({ "deserializeError": e.to_string() }))
}

/// Invoke a command that returns nothing and just check it does not error.
pub fn call_ok(app: &App, cmd: &str, args: serde_json::Value) {
    call::<serde_json::Value>(app, cmd, args).unwrap_or_else(|e| panic!("{cmd} failed: {e}"));
}

/// A JSON `Location` for the local disk.
pub fn local(path: &str) -> serde_json::Value {
    serde_json::json!({ "fs": "local", "path": path })
}

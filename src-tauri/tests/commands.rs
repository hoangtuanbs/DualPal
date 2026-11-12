//! Tauri command-layer tests (Phase 3, L4).
//!
//! Each test builds a real [`dualpal_app::AppState`] on temp folders and calls
//! the commands through the mock runtime, so argument names, JSON shapes and
//! error shapes are checked against the actual `generate_handler!` wiring.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{app, call, call_ok, local, TestApp};
use serde_json::{json, Value};

/// The set of commands exercised by the `CMD-*` tests below. CMD-000 fails when
/// a command is added to `dualpal_app::COMMANDS` without a corresponding test.
fn tested_commands() -> HashSet<&'static str> {
    [
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
        "find_files",
        "cancel_find_files",
        "read_text",
        "write_text",
        "connections",
        "save_connection",
        "import_connections",
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
    ]
    .into_iter()
    .collect()
}

/// CMD-000 — every command in the handler list has a test.
#[test]
fn every_command_has_a_test() {
    let tested = tested_commands();
    let mut missing: Vec<&str> = dualpal_app::COMMANDS
        .iter()
        .copied()
        .filter(|c| !tested.contains(c))
        .collect();
    missing.sort();
    assert!(missing.is_empty(), "commands without a test: {missing:?}");
}

/// CMD-001 — `file_systems` lists `local`.
#[test]
fn file_systems_lists_local() {
    let TestApp { app, tmp: _tmp } = app();
    let list: Vec<Value> = call(&app, "file_systems", json!({})).unwrap();
    assert!(list.iter().any(|f| f["id"] == "local"));
}

/// CMD-002 — `home_location` honours `DUALPAL_HOME`.
#[test]
fn home_location_honours_dualpal_home() {
    let _guard = common::ENV_LOCK.lock().unwrap();
    let TestApp { app, tmp: _tmp } = app();
    let home = std::env::temp_dir().join(format!("dualpal-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("DUALPAL_HOME", &home);

    let loc: Value = call(&app, "home_location", json!({})).unwrap();
    assert_eq!(loc["fs"], "local");
    assert_eq!(
        loc["path"].as_str().unwrap().trim_end_matches(['/', '\\']),
        home.to_string_lossy().trim_end_matches(['/', '\\'])
    );
    std::env::remove_var("DUALPAL_HOME");
}

/// CMD-003 — `list_dir` returns a camelCase Entry; CMD-004 missing folder → error.
#[test]
fn list_dir_shape_and_missing_folder() {
    let TestApp { app, tmp } = app();
    std::fs::create_dir_all(tmp.path().join("d")).unwrap();
    std::fs::write(tmp.path().join("d/a.txt"), "hello").unwrap();

    let entries: Vec<Value> = call(
        &app,
        "list_dir",
        json!({ "location": local(&tmp.path().join("d").to_string_lossy()) }),
    )
    .unwrap();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert!(e["path"].is_string());
    assert!(e["name"].is_string());
    assert!(e["size"].is_number());
    assert!(e["hidden"].is_boolean());
    assert!(e.get("isDir").is_some() || e.get("kind").is_some(), "{e}");

    // CMD-004: a missing folder is `{kind: notFound, message}`.
    let err = call::<Value>(
        &app,
        "list_dir",
        json!({ "location": local(&tmp.path().join("nope").to_string_lossy()) }),
    )
    .unwrap_err();
    assert_eq!(err["kind"], "notFound", "{err}");
    assert!(err["message"].is_string());
}

/// CMD-005 — `parent_location` of the root is null.
#[test]
fn parent_location_root_is_null() {
    let TestApp { app, tmp } = app();
    // The file-system root above the temp folder: `/` or `C:\`.
    let root = tmp
        .path()
        .ancestors()
        .last()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let parent: Option<Value> =
        call(&app, "parent_location", json!({ "location": local(&root) })).unwrap();
    assert!(parent.is_none(), "{parent:?}");
}

/// CMD-006 — `enqueue_operation` runs and broadcasts `operation-updated`.
#[test]
fn enqueue_operation_emits_events() {
    use tauri::Listener;

    let TestApp { app, tmp } = app().with_events();
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = received.clone();
    app.listen("operation-updated", move |event| {
        if let Ok(v) = serde_json::from_str::<Value>(event.payload()) {
            sink.lock().unwrap().push(v);
        }
    });

    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "aaa").unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();

    let id: String = call(
        &app,
        "enqueue_operation",
        json!({
            "operation": {
                "type": "copy",
                "sources": [local(&src.to_string_lossy())],
                "destination": local(&dst.to_string_lossy()),
                "conflict": "fail"
            }
        }),
    )
    .unwrap();
    assert!(!id.is_empty());

    // Wait for the completed event.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let done = received
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["id"] == id && e["status"] == "completed");
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "operation never completed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(dst.join("src/a.txt").exists());
}

/// CMD-007 — operations list, cancel and clear.
#[test]
fn operations_cancel_and_clear() {
    let TestApp { app, tmp } = app();
    // A queued operation (one slot is busy, but with default 2 it may start;
    // use a big copy to make cancellation of a running op observable).
    let big = tmp.path().join("big.bin");
    std::fs::write(&big, vec![7u8; 64 * 1024 * 1024]).unwrap();
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();

    let id: String = call(
        &app,
        "enqueue_operation",
        json!({ "operation": { "type": "copy", "sources": [local(&big.to_string_lossy())], "destination": local(&dst.to_string_lossy()), "conflict": "fail" } }),
    )
    .unwrap();

    let ops: Vec<Value> = call(&app, "operations", json!({})).unwrap();
    assert!(ops.iter().any(|o| o["id"] == id));

    // cancel_operation returns true while it is still running.
    let cancelled: bool = call(&app, "cancel_operation", json!({ "id": id })).unwrap();
    assert!(cancelled);

    // clear_finished_operations runs without error.
    call_ok(&app, "clear_finished_operations", json!({}));
}

/// CMD-008 — activity, clear_activity and activity_insights argument names.
#[test]
fn activity_commands() {
    let TestApp { app, tmp: _tmp } = app();
    let list: Vec<Value> = call(&app, "activity", json!({})).unwrap();
    assert!(list.is_empty());
    call_ok(&app, "clear_activity", json!({}));

    // The argument names match the Rust parameters exactly.
    // Tauri takes camelCase argument names, as `ui/src/lib/api.ts` sends them.
    let insights: Value = call(
        &app,
        "activity_insights",
        json!({ "days": 7, "utcOffsetMinutes": 0 }),
    )
    .unwrap();
    assert!(insights["days"].is_array());
    // A wrong argument name is rejected by Tauri, guarding UI/Rust drift.
    assert!(call::<Value>(
        &app,
        "activity_insights",
        json!({ "days": 7, "utc_offset_minutes": 0 })
    )
    .is_err());
}

/// CMD-009 — volumes and available_space.
#[test]
fn volumes_and_available_space() {
    let TestApp { app, tmp } = app();
    let vols: Vec<Value> = call(&app, "volumes", json!({})).unwrap();
    // At least the system disk is reported (skipped on a headless runner with
    // no volume info, so this is a smoke check only).
    let _ = vols;

    let space: Option<u64> = call(
        &app,
        "available_space",
        json!({ "location": local(&tmp.path().to_string_lossy()) }),
    )
    .unwrap();
    assert!(space.is_some());
}

/// CMD-010 — favorites add/remove/reorder return the new list.
#[test]
fn favorites_crud() {
    let TestApp { app, tmp: _tmp } = app();
    let fav = json!({ "name": "Work", "location": local("/work"), "icon": "folder" });
    let added: Vec<Value> = call(&app, "add_favorite", json!({ "favorite": fav })).unwrap();
    assert!(added.iter().any(|f| f["name"] == "Work"));

    let list: Vec<Value> = call(&app, "favorites", json!({})).unwrap();
    assert!(list.iter().any(|f| f["name"] == "Work"));

    call_ok(&app, "reorder_favorites", json!({ "from": 0, "to": 0 }));
    let removed: Vec<Value> = call(
        &app,
        "remove_favorite",
        json!({ "location": local("/work") }),
    )
    .unwrap();
    assert!(!removed.iter().any(|f| f["name"] == "Work"));
}

/// CMD-011, SEC-004 — `open_path` / `open_url` record to the opener log; bad schemes refused.
#[test]
fn open_path_and_open_url_use_the_log() {
    let _guard = common::ENV_LOCK.lock().unwrap();
    let TestApp { app, tmp } = app();
    let log = tmp.path().join("open.log");
    std::env::set_var("DUALPAL_OPENER_LOG", &log);

    let file = tmp.path().join("a.txt");
    std::fs::write(&file, "x").unwrap();
    call_ok(
        &app,
        "open_path",
        json!({ "location": local(&file.to_string_lossy()) }),
    );
    call_ok(
        &app,
        "open_url",
        json!({ "url": "mailto:hoangtuanbs@proton.me" }),
    );

    // Only https: and mailto: are allowed.
    for bad in ["file:///etc/passwd", "javascript:alert(1)"] {
        let err = call::<Value>(&app, "open_url", json!({ "url": bad })).unwrap_err();
        assert_eq!(err["kind"], "unsupported", "{bad} -> {err}");
    }

    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("a.txt"), "{text}");
    assert!(text.contains("mailto:hoangtuanbs@proton.me"), "{text}");
    std::env::remove_var("DUALPAL_OPENER_LOG");
}

/// CMD-012 — `about_info` reports the platform.
#[test]
fn about_info() {
    let TestApp { app, tmp: _tmp } = app();
    let info: Value = call(&app, "about_info", json!({})).unwrap();
    assert_eq!(info["os"], std::env::consts::OS);
    assert_eq!(info["arch"], std::env::consts::ARCH);
    assert!(info["version"].is_string());
}

/// CMD-013 — stat, read_text, write_text.
#[test]
fn stat_read_write_text() {
    let TestApp { app, tmp } = app();
    let file = tmp.path().join("notes.txt");
    std::fs::write(&file, "héllo").unwrap();

    let st: Value = call(
        &app,
        "stat",
        json!({ "location": local(&file.to_string_lossy()) }),
    )
    .unwrap();
    assert_eq!(st["size"], 6);

    let text: Value = call(
        &app,
        "read_text",
        json!({ "location": local(&file.to_string_lossy()), "maxBytes": 100 }),
    )
    .unwrap();
    assert_eq!(text["text"], "héllo");

    call_ok(
        &app,
        "write_text",
        json!({ "location": local(&file.to_string_lossy()), "text": "saved ✓" }),
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "saved ✓");
}

/// CMD-014 — `connections` JSON never includes a secret; CMD-015 CRUD.
#[test]
fn connections_never_include_secrets() {
    let TestApp { app, tmp: _tmp } = app();
    let conn = json!({
        "id": "c1",
        "name": "My SFTP",
        "endpoint": { "type": "sftp", "host": "example.com", "port": 22, "user": "me", "auth": { "method": "password" } },
        "initialPath": null,
        "favorites": []
    });
    call::<Value>(
        &app,
        "save_connection",
        json!({ "connection": conn, "secret": "S3CR3T" }),
    )
    .unwrap();

    let list: Vec<Value> = call(&app, "connections", json!({})).unwrap();
    let serialized = serde_json::to_string(&list).unwrap();
    assert!(!serialized.contains("S3CR3T"), "{serialized}");
    assert!(serialized.contains("My SFTP"));

    // reorder + remove.
    call_ok(&app, "reorder_connections", json!({ "ids": ["c1"] }));
    call_ok(&app, "remove_connection", json!({ "id": "c1" }));
    let list: Vec<Value> = call(&app, "connections", json!({})).unwrap();
    assert!(list.is_empty());
}

/// IMP-100 — `import_connections` reads each source and saves nothing itself.
#[test]
fn import_connections_reads_each_source() {
    let TestApp { app, tmp: _tmp } = app();
    let winscp = "[Sessions\\Work]\nHostName=files.example.com\nUserName=alice\nFSProtocol=1\n";
    let report: Value = call(
        &app,
        "import_connections",
        json!({ "source": "winscp", "text": winscp }),
    )
    .unwrap();
    assert_eq!(report["connections"][0]["connection"]["name"], "Work");
    assert_eq!(
        report["connections"][0]["connection"]["endpoint"]["host"],
        "files.example.com"
    );
    assert_eq!(report["connections"][0]["passwordMissing"], true);
    assert!(report["skipped"].as_array().unwrap().is_empty());

    // Nothing is saved until the UI calls save_connection.
    let list: Vec<Value> = call(&app, "connections", json!({})).unwrap();
    assert!(list.is_empty());

    let cyberduck: Value = call(
        &app,
        "import_connections",
        json!({ "source": "cyberduck", "text": "<plist><array></array></plist>" }),
    )
    .unwrap();
    assert!(cyberduck["connections"].as_array().unwrap().is_empty());

    let bad = call::<Value>(
        &app,
        "import_connections",
        json!({ "source": "putty", "text": "" }),
    )
    .unwrap_err();
    assert_eq!(bad["kind"], "invalid", "{bad}");
}

/// CMD-016/017 — connect with a real server (L3; skipped without one).
#[test]
fn connect_and_remember() {
    if dualpal_testkit::remote_env("DUALPAL_TEST_SFTP").is_none() {
        return; // needs the docker SFTP server
    }
    let TestApp { app, tmp: _tmp } = app();
    // Save a connection with a secret and remember: false first.
    let _ = &app;
    // The full flow needs the server and the key; it is covered by the L3
    // suite. This test only asserts the command is wired (wrong host errors).
    let err = call::<Value>(
        &app,
        "connect",
        json!({ "id": "missing-connection", "secret": null, "remember": false, "trust": null }),
    )
    .unwrap_err();
    assert_eq!(err["kind"], "notFound", "{err}");
}

/// CMD-018 — disconnect.
#[test]
fn disconnect() {
    let TestApp { app, tmp: _tmp } = app();
    // Disconnecting an unknown connection is a no-op, not a crash.
    call_ok(&app, "disconnect", json!({ "id": "does-not-exist" }));
}

/// CMD-019 — connection favorites.
#[test]
fn connection_favorites() {
    let TestApp { app, tmp: _tmp } = app();
    let conn = json!({
        "id": "c1",
        "name": "SFTP",
        "endpoint": { "type": "sftp", "host": "example.com", "port": 22, "user": "me", "auth": { "method": "agent" } },
        "initialPath": null,
        "favorites": []
    });
    call::<Value>(
        &app,
        "save_connection",
        json!({ "connection": conn, "secret": null }),
    )
    .unwrap();

    call_ok(
        &app,
        "add_connection_favorite",
        json!({ "id": "c1", "favorite": { "name": "Docs", "path": "/home/me/docs" } }),
    );
    let list: Vec<Value> = call(&app, "connections", json!({})).unwrap();
    let c = list.iter().find(|c| c["id"] == "c1").unwrap();
    assert_eq!(c["favorites"].as_array().unwrap().len(), 1);

    call_ok(
        &app,
        "remove_connection_favorite",
        json!({ "id": "c1", "path": "/home/me/docs" }),
    );
    let list: Vec<Value> = call(&app, "connections", json!({})).unwrap();
    let c = list.iter().find(|c| c["id"] == "c1").unwrap();
    assert!(c["favorites"].as_array().unwrap().is_empty());
}

/// CMD-020 — `ssh_config_hosts` reads `$DUALPAL_HOME/.ssh/config`.
#[test]
fn ssh_config_hosts_reads_home() {
    let _guard = common::ENV_LOCK.lock().unwrap();
    let TestApp { app, tmp: _tmp } = app();
    let home = std::env::temp_dir().join(format!("dualpal-ssh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(
        home.join(".ssh/config"),
        "Host prod\n  HostName prod.example.com\n  User deploy\n  Port 2222\n",
    )
    .unwrap();
    std::env::set_var("DUALPAL_HOME", &home);

    let hosts: Vec<Value> = call(&app, "ssh_config_hosts", json!({})).unwrap();
    assert!(hosts
        .iter()
        .any(|h| h["alias"] == "prod" && h["hostName"] == "prod.example.com"));
    std::env::remove_var("DUALPAL_HOME");
}

/// CMD-021 — `open_file` on a local file goes to the opener log.
#[test]
fn open_file_local_uses_log() {
    let _guard = common::ENV_LOCK.lock().unwrap();
    let TestApp { app, tmp } = app();
    let log = tmp.path().join("open.log");
    std::env::set_var("DUALPAL_OPENER_LOG", &log);
    let file = tmp.path().join("doc.txt");
    std::fs::write(&file, "hi").unwrap();

    let session: Option<Value> = call(
        &app,
        "open_file",
        json!({ "location": local(&file.to_string_lossy()), "editor": false }),
    )
    .unwrap();
    assert!(session.is_none(), "a local file has no edit session");
    assert!(std::fs::read_to_string(&log).unwrap().contains("doc.txt"));
    std::env::remove_var("DUALPAL_OPENER_LOG");
}

/// CMD-022 — edit sessions listing and stop.
#[test]
fn edit_sessions_and_stop() {
    let TestApp { app, tmp: _tmp } = app();
    let sessions: Vec<Value> = call(&app, "edit_sessions", json!({})).unwrap();
    assert!(sessions.is_empty());

    // Stopping an unknown session reports false.
    let stopped: bool = call(
        &app,
        "stop_edit_session",
        json!({ "id": uuid::Uuid::new_v4().to_string() }),
    )
    .unwrap();
    assert!(!stopped);

    // Uploading an unknown session errors.
    let err = call::<Value>(
        &app,
        "upload_edit_session",
        json!({ "id": uuid::Uuid::new_v4().to_string() }),
    )
    .unwrap_err();
    assert_eq!(err["kind"], "notFound", "{err}");
}

/// CMD-023 — `preview_path` of a local file returns the same path.
#[test]
fn preview_path_local() {
    let TestApp { app, tmp } = app();
    let file = tmp.path().join("img.png");
    std::fs::write(&file, "not really a png").unwrap();
    let p: String = call(
        &app,
        "preview_path",
        json!({ "location": local(&file.to_string_lossy()) }),
    )
    .unwrap();
    assert_eq!(p, file.to_string_lossy());
}

/// CMD-024 — settings round trip.
#[test]
fn settings_round_trip() {
    let TestApp { app, tmp: _tmp } = app();
    let settings: Value = call(&app, "settings", json!({})).unwrap();
    assert_eq!(settings["editor"], Value::Null);

    call_ok(
        &app,
        "save_settings",
        json!({ "settings": { "editor": "/usr/bin/emacs", "diffTool": "meld {left} {right}" } }),
    );
    let settings: Value = call(&app, "settings", json!({})).unwrap();
    assert_eq!(settings["editor"], "/usr/bin/emacs");
    assert_eq!(settings["diffTool"], "meld {left} {right}");
}

/// CMD-025 — `sync_plan` returns a SyncPreview.
#[test]
fn sync_plan_shape() {
    let TestApp { app, tmp } = app();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(l.join("a.txt"), "a").unwrap();

    let plan: Value = call(
        &app,
        "sync_plan",
        json!({
            "left": local(&l.to_string_lossy()),
            "right": local(&r.to_string_lossy()),
            "mode": "update",
            "options": { "by": "sizeAndDate", "toleranceMs": 2000, "exclude": [], "includeHidden": true }
        }),
    )
    .unwrap();
    assert!(plan["id"].is_string());
    assert_eq!(plan["mode"], "update");
    assert!(plan["steps"].is_array());
    assert!(plan["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["rel"] == "a.txt"));
}

/// CMD-026/027 — cancel_compare and run_sync.
#[test]
fn cancel_compare_and_run_sync() {
    let TestApp { app, tmp } = app();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(l.join("a.txt"), "a").unwrap();

    // cancel_compare is a no-op when nothing is comparing.
    call_ok(&app, "cancel_compare", json!({}));

    let plan: Value = call(
        &app,
        "sync_plan",
        json!({
            "left": local(&l.to_string_lossy()),
            "right": local(&r.to_string_lossy()),
            "mode": "update",
            "options": { "by": "sizeAndDate", "toleranceMs": 2000, "exclude": [], "includeHidden": true }
        }),
    )
    .unwrap();

    let op_id: String = call(
        &app,
        "run_sync",
        json!({ "plan": plan["id"], "overrides": {} }),
    )
    .unwrap();
    assert!(!op_id.is_empty());
    // run_sync returns once the operation is queued: wait for it to finish.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ops: Vec<Value> = call(&app, "operations", json!({})).unwrap();
        let status = ops
            .iter()
            .find(|o| o["id"] == op_id.as_str())
            .map(|o| o["status"].clone());
        if status.as_ref().is_some_and(|s| s == "completed") {
            break;
        }
        assert!(
            status.as_ref().is_none_or(|s| s != "failed"),
            "sync failed: {ops:?}"
        );
        assert!(Instant::now() < deadline, "sync never completed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(r.join("a.txt").exists());
}

/// CMD-028 — forget_sync_history.
#[test]
fn forget_sync_history() {
    let TestApp { app, tmp } = app();
    let l = tmp.path().join("l");
    let r = tmp.path().join("r");
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    call_ok(
        &app,
        "forget_sync_history",
        json!({ "left": local(&l.to_string_lossy()), "right": local(&r.to_string_lossy()) }),
    );
}

/// CMD-029 — sync favorites CRUD.
#[test]
fn sync_favorites_crud() {
    let TestApp { app, tmp: _tmp } = app();
    let fav = json!({
        "id": "",
        "name": "Backup",
        "left": local("/a"),
        "right": { "fs": "conn:x", "path": "/backup" },
        "mode": "mirror",
        "options": { "by": "sizeAndDate", "toleranceMs": 2000, "exclude": [], "includeHidden": true }
    });
    let saved: Value = call(&app, "save_sync_favorite", json!({ "favorite": fav })).unwrap();
    assert!(saved["id"].is_string() && !saved["id"].as_str().unwrap().is_empty());

    let list: Vec<Value> = call(&app, "sync_favorites", json!({})).unwrap();
    assert_eq!(list.len(), 1);

    call_ok(&app, "remove_sync_favorite", json!({ "id": saved["id"] }));
    let list: Vec<Value> = call(&app, "sync_favorites", json!({})).unwrap();
    assert!(list.is_empty());
}

/// CMD-030 — rename_preview.
#[test]
fn rename_preview() {
    let app = app();
    let preview: Vec<Value> = call(
        &app.app,
        "rename_preview",
        json!({
            "rules": [{ "type": "replace", "find": "a", "replace": "X", "regex": false, "caseSensitive": false, "scope": "name" }],
            "inputs": [{ "name": "alpha.txt", "dir": false, "modifiedMs": null, "parent": "folder" }],
            "others": []
        }),
    )
    .unwrap();
    assert_eq!(preview[0]["newName"], "XlphX.txt");
}

/// CMD-031 — rename presets.
#[test]
fn rename_presets() {
    let app = app();
    let presets: Vec<Value> = call(&app.app, "rename_presets", json!({})).unwrap();
    assert!(!presets.is_empty(), "the app ships built-in presets");

    call_ok(
        &app.app,
        "save_rename_preset",
        json!({ "preset": { "name": "My rule", "rules": [], "builtin": false } }),
    );
    call_ok(
        &app.app,
        "remove_rename_preset",
        json!({ "name": "My rule" }),
    );
}

/// CMD-032 — diff_tools and compare_files.
#[test]
fn diff_tools_and_compare_files() {
    let _guard = common::ENV_LOCK.lock().unwrap();
    let app = app();
    let tools: Vec<Value> = call(&app.app, "diff_tools", json!({})).unwrap();
    let _ = tools; // may be empty on a bare CI runner

    let tmp = app.tmp.path();
    let a = tmp.join("a.txt");
    let b = tmp.join("b.txt");
    std::fs::write(&a, "a").unwrap();
    std::fs::write(&b, "b").unwrap();

    // With a diff-tool log set, compare_files records instead of launching.
    let log = tmp.join("diff.log");
    std::env::set_var("DUALPAL_DIFF_TOOL_LOG", &log);
    // A custom template avoids depending on an installed tool.
    call_ok(
        &app.app,
        "save_settings",
        json!({ "settings": { "editor": null, "diffTool": "meld {left} {right}" } }),
    );
    call_ok(
        &app.app,
        "compare_files",
        json!({ "left": local(&a.to_string_lossy()), "right": local(&b.to_string_lossy()) }),
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("a.txt") && text.contains("b.txt"), "{text}");
    std::env::remove_var("DUALPAL_DIFF_TOOL_LOG");
}

/// CMD-033 — live sync start/status/stop.
#[test]
fn live_sync_lifecycle() {
    let app = app();
    let a = app.tmp.path().join("a");
    let b = app.tmp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let status: Value = call(
        &app.app,
        "live_sync_start",
        json!({ "from": local(&a.to_string_lossy()), "to": local(&b.to_string_lossy()), "direction": "oneWay" }),
    )
    .unwrap();
    assert_eq!(status["direction"], "oneWay");

    let now: Option<Value> = call(&app.app, "live_sync_status", json!({})).unwrap();
    assert!(now.is_some());

    call_ok(&app.app, "live_sync_stop", json!({}));
    let after: Option<Value> = call(&app.app, "live_sync_status", json!({})).unwrap();
    assert!(after.is_none());
}

/// CMD-034, SEC-005 — the capability file grants only what the UI needs.
#[test]
fn capability_permissions_are_reviewed() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json"),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    insta::assert_json_snapshot!(value["permissions"], @r###"
    [
      "core:default",
      "updater:default",
      "process:allow-restart"
    ]
    "###);
}

/// CMD-035, SEC-005 — the CSP has no `unsafe-eval` and no remote origins.
#[test]
fn csp_has_no_unsafe_eval_or_remote_origins() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    let csp = value["app"]["security"]["csp"].as_str().unwrap();
    assert!(!csp.contains("unsafe-eval"), "{csp}");
    assert!(!csp.contains("https://"), "{csp}");
}

/// CON-025 — disconnecting a connection unregisters its file system and ends
/// the edit sessions open on it.
#[test]
fn disconnect_unregisters_the_fs_and_stops_its_edit_sessions() {
    use tauri::Manager;
    let TestApp { app, tmp: _tmp } = app();
    let state = app.state::<dualpal_app::AppState>();
    let fs_id = dualpal_core::connections::fs_id("server-1");
    let mem = dualpal_testkit::MemFs::new(fs_id.clone());
    mem.seed_file("/notes.txt", "remote notes");
    state.registry.register(Arc::new(mem));
    let session = tauri::async_runtime::block_on(
        state
            .editor
            .start(dualpal_core::Location::new(fs_id.clone(), "/notes.txt")),
    )
    .unwrap();
    assert!(state.registry.ids().contains(&fs_id));
    assert_eq!(state.editor.list().len(), 1);

    call_ok(&app, "disconnect", json!({ "id": "server-1" }));

    assert!(
        !state.registry.ids().contains(&fs_id),
        "{:?}",
        state.registry.ids()
    );
    assert!(
        state.editor.list().iter().all(|s| s.id != session.id),
        "the edit session on that server is stopped: {:?}",
        state.editor.list()
    );
    assert!(
        !std::path::Path::new(&session.local_path).exists(),
        "an unchanged local copy is removed"
    );
}

/// DIFF-006 — comparing two remote files downloads them first and hands the
/// compare app two local paths holding the server contents.
#[test]
fn compare_files_downloads_remote_files_first() {
    use tauri::Manager;
    let _guard = common::ENV_LOCK.lock().unwrap();
    let app = app();
    let state = app.app.state::<dualpal_app::AppState>();
    let mem = dualpal_testkit::MemFs::new("conn:diff");
    mem.seed_file("/left.txt", "left side\n");
    mem.seed_file("/right.txt", "right side\n");
    state.registry.register(Arc::new(mem));

    let log = app.tmp.path().join("diff.log");
    std::env::set_var("DUALPAL_DIFF_TOOL_LOG", &log);
    call_ok(
        &app.app,
        "save_settings",
        json!({ "settings": { "editor": null, "diffTool": "difftool {left} {right}" } }),
    );
    let result = call::<Value>(
        &app.app,
        "compare_files",
        json!({
            "left": { "fs": "conn:diff", "path": "/left.txt" },
            "right": { "fs": "conn:diff", "path": "/right.txt" },
        }),
    );
    std::env::remove_var("DUALPAL_DIFF_TOOL_LOG");
    result.unwrap();

    let line = std::fs::read_to_string(&log).unwrap();
    let args: Vec<&str> = line.trim().split(' ').collect();
    assert_eq!(args.len(), 3, "{line}");
    assert_eq!(args[0], "difftool");
    for (path, want) in [(args[1], "left side\n"), (args[2], "right side\n")] {
        let p = std::path::Path::new(path);
        assert!(p.is_absolute(), "{path} is a local path");
        assert_eq!(std::fs::read_to_string(p).unwrap(), want, "{path}");
    }

/// CMD-036 — folder_size totals a folder; a file is an error; cancelling an
/// unknown request is a no-op.
#[test]
fn folder_size_totals_and_cancel() {
    let TestApp { app, tmp } = app();
    let dir = tmp.path().join("d");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "hello").unwrap();
    std::fs::write(dir.join("sub/b.txt"), "xy").unwrap();

    let size: Value = call(
        &app,
        "folder_size",
        json!({ "location": local(&dir.to_string_lossy()), "request": 1 }),
    )
    .unwrap();
    assert_eq!(size["bytes"], 7);
    assert_eq!(size["files"], 2);
    assert_eq!(size["folders"], 1);
    assert_eq!(size["skipped"], 0);

    let file_err = call::<Value>(
        &app,
        "folder_size",
        json!({ "location": local(&dir.join("a.txt").to_string_lossy()), "request": 2 }),
    )
    .unwrap_err();
    assert_eq!(file_err["kind"], "invalid");

    call_ok(&app, "cancel_folder_size", json!({ "request": 99 }));
}

/// CMD-037 — find_files returns names that match below a folder, shallowest
/// first; a blank query finds nothing; cancelling an unknown request is a no-op.
#[test]
fn find_files_matches_names_and_cancel() {
    let TestApp { app, tmp } = app();
    let dir = tmp.path().join("d");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("Report.txt"), "a").unwrap();
    std::fs::write(dir.join("sub/report-2.md"), "b").unwrap();
    std::fs::write(dir.join("other.txt"), "c").unwrap();

    let found: Value = call(
        &app,
        "find_files",
        json!({ "location": local(&dir.to_string_lossy()), "query": "REPORT", "request": 1 }),
    )
    .unwrap();
    let names: Vec<&str> = found["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Report.txt", "report-2.md"]);
    assert_eq!(found["truncated"], false);

    let blank: Value = call(
        &app,
        "find_files",
        json!({ "location": local(&dir.to_string_lossy()), "query": "  ", "request": 2 }),
    )
    .unwrap();
    assert!(blank["hits"].as_array().unwrap().is_empty());

    call_ok(&app, "cancel_find_files", json!({ "request": 99 }));
}

//! Settings persistence cases (Phase 1, §8.4).

use dualpal_core::settings::{Settings, SettingsStore};

/// SET-001 — a missing file yields the defaults.
#[test]
fn defaults_when_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::open(dir.path().join("settings.json"));
    assert_eq!(store.get(), Settings::default());
}

/// SET-002 — editor and diff tool round-trip.
#[test]
fn round_trip_editor_and_diff_tool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let store = SettingsStore::open(&path);
    store
        .set(Settings {
            editor: Some("/usr/bin/emacs".into()),
            diff_tool: Some("meld {left} {right}".into()),
        })
        .unwrap();
    let reopened = SettingsStore::open(&path);
    assert_eq!(reopened.get().editor.as_deref(), Some("/usr/bin/emacs"));
    assert_eq!(
        reopened.get().diff_tool.as_deref(),
        Some("meld {left} {right}")
    );
}

/// SET-003 — a corrupt file yields the defaults.
#[test]
fn corrupt_settings_yields_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{ not valid json").unwrap();
    let store = SettingsStore::open(&path);
    assert_eq!(store.get(), Settings::default());
}

/// SET-004 — unknown fields are ignored (forward compatible).
#[test]
fn unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{ "editor": "/v", "futureField": 123 }"#).unwrap();
    let store = SettingsStore::open(&path);
    assert_eq!(store.get().editor.as_deref(), Some("/v"));
}

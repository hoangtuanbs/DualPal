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
            ..Settings::default()
        })
        .unwrap();
    let reopened = SettingsStore::open(&path);
    assert_eq!(reopened.get().editor.as_deref(), Some("/usr/bin/emacs"));
    assert_eq!(
        reopened.get().diff_tool.as_deref(),
        Some("meld {left} {right}")
    );
}

/// SET-005 — the last update check time round-trips, and an older file without it still loads.
#[test]
fn last_update_check_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{ "editor": "/v" }"#).unwrap();
    assert_eq!(SettingsStore::open(&path).get().last_update_check, None);

    let store = SettingsStore::open(&path);
    store
        .set(Settings {
            last_update_check: Some(1_794_328_406_642),
            ..store.get()
        })
        .unwrap();
    let reopened = SettingsStore::open(&path);
    assert_eq!(reopened.get().last_update_check, Some(1_794_328_406_642));
    assert_eq!(reopened.get().editor.as_deref(), Some("/v"));
}

/// SET-006 — the update check switch and the skipped release round-trip.
#[test]
fn update_check_preferences_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{ "editor": "/v" }"#).unwrap();
    let old = SettingsStore::open(&path).get();
    assert_eq!(old.update_checks, None);
    assert_eq!(old.skipped_version, None);

    SettingsStore::open(&path)
        .set(Settings {
            update_checks: Some(false),
            skipped_version: Some("0.3.0".into()),
            ..old
        })
        .unwrap();
    let json = std::fs::read_to_string(&path).unwrap();
    assert!(json.contains("\"updateChecks\": false"), "{json}");
    assert!(json.contains("\"skippedVersion\": \"0.3.0\""), "{json}");
    let reopened = SettingsStore::open(&path).get();
    assert_eq!(reopened.update_checks, Some(false));
    assert_eq!(reopened.skipped_version.as_deref(), Some("0.3.0"));
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

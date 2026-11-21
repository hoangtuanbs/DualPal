//! Settings persistence cases (Phase 1, §8.4).

use dualpal_core::settings::{
    Density, FontSize, PaneStart, Settings, SettingsStore, StartMode, Theme,
};
use serde_json::json;

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

/// SET-007 — the Settings screen's choices round-trip in camelCase.
#[test]
fn settings_screen_choices_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let chosen = Settings {
        theme: Theme::Dark,
        font_size: FontSize::Large,
        density: Density::Compact,
        show_hidden: true,
        hidden_columns: vec!["size".into()],
        pane_start: vec![
            PaneStart {
                mode: StartMode::Last,
                folder: None,
            },
            PaneStart {
                mode: StartMode::Folder,
                folder: Some("/srv/data".into()),
            },
        ],
        last_folders: vec![Some("/home/me/docs".into()), None],
        key_bindings: [("copy".to_string(), vec!["Mod+Shift+C".to_string()])]
            .into_iter()
            .collect(),
        ..Settings::default()
    };
    SettingsStore::open(&path).set(chosen.clone()).unwrap();
    let json = std::fs::read_to_string(&path).unwrap();
    for part in [
        r#""theme": "dark""#,
        r#""fontSize": "large""#,
        r#""density": "compact""#,
        r#""showHidden": true"#,
        r#""hiddenColumns""#,
        r#""paneStart""#,
        r#""mode": "folder""#,
        r#""keyBindings""#,
        r#""Mod+Shift+C""#,
    ] {
        assert!(json.contains(part), "{part} missing in {json}");
    }
    assert_eq!(SettingsStore::open(&path).get(), chosen);
}

/// SET-008 — a file from before the Settings screen loads with today's
/// behaviour, and a choice from a newer version reads as the default.
#[test]
fn older_and_newer_files_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{ "editor": "/v", "diffTool": "meld {left} {right}", "updateChecks": false }"#,
    )
    .unwrap();
    let old = SettingsStore::open(&path).get();
    assert_eq!(old.editor.as_deref(), Some("/v"));
    assert_eq!(old.update_checks, Some(false));
    assert_eq!(old.theme, Theme::System);
    assert_eq!(old.font_size, FontSize::Normal);
    assert_eq!(old.density, Density::Normal);
    assert!(!old.show_hidden);
    assert!(old.hidden_columns.is_empty());
    assert!(old.pane_start.is_empty());
    assert!(old.key_bindings.is_empty());

    std::fs::write(
        &path,
        r#"{ "editor": "/v", "theme": "solarized", "density": "spacious",
             "paneStart": [{ "mode": "cloud" }] }"#,
    )
    .unwrap();
    let newer = SettingsStore::open(&path).get();
    assert_eq!(newer.editor.as_deref(), Some("/v"));
    assert_eq!(newer.theme, Theme::System);
    assert_eq!(newer.density, Density::Normal);
    assert_eq!(newer.pane_start[0].mode, StartMode::Home);
}

/// SET-009 — `update` changes only the fields given and keeps the rest.
#[test]
fn update_changes_only_given_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let store = SettingsStore::open(&path);
    store
        .set(Settings {
            editor: Some("/usr/bin/emacs".into()),
            last_update_check: Some(5),
            ..Settings::default()
        })
        .unwrap();
    let next = store
        .update(json!({ "theme": "light", "keyBindings": { "refresh": ["F9"] } }))
        .unwrap();
    assert_eq!(next.theme, Theme::Light);
    assert_eq!(next.editor.as_deref(), Some("/usr/bin/emacs"));
    assert_eq!(next.last_update_check, Some(5));
    assert_eq!(next.key_bindings["refresh"], vec!["F9".to_string()]);
    assert_eq!(SettingsStore::open(&path).get(), next);

    // A wrongly shaped change is refused and changes nothing.
    assert!(store.update(json!({ "showHidden": "yes" })).is_err());
    assert!(store.update(json!(["theme"])).is_err());
    assert_eq!(SettingsStore::open(&path).get(), next);
}

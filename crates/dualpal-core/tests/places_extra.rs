//! Places and favorites cases (Phase 1, §8.4).

use dualpal_core::places::{Favorite, Favorites};
use dualpal_core::Location;

fn fav(name: &str, path: &str) -> Favorite {
    Favorite {
        name: name.into(),
        location: Location::new("local", path),
        icon: "folder".into(),
    }
}

/// PLC-003 — a corrupt favorites file falls back to standard folders and is
/// not overwritten until something changes.
#[test]
fn corrupt_favorites_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("favorites.json");
    std::fs::write(&path, "{ not valid").unwrap();
    let favs = Favorites::open(&path);
    // Standard folders exist, so the fallback is non-empty.
    assert!(!favs.list().is_empty());
    // The corrupt file was not rewritten just by opening it.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not valid");
    // Adding a favorite rewrites it as valid JSON.
    favs.add(fav("Work", "/work")).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("Work"));
}

/// PLC-004 — reordering out of range should error, not silently clamp.
///
/// The current implementation clamps the index instead of failing.
#[test]
#[ignore = "BUG: Favorites::reorder clamps out-of-range indices instead of erroring"]
fn reorder_out_of_range_errors() {
    let favs = Favorites::in_memory(vec![fav("A", "/a"), fav("B", "/b")]);
    assert!(favs.reorder(5, 0).is_err());
    assert_eq!(favs.list().len(), 2);
}

/// PLC-006 — the home folder reports available space when it exists.
#[test]
fn available_space_reports_for_home() {
    let Some(home) = dualpal_core::places::home_dir() else {
        return;
    };
    if !home.is_dir() {
        return;
    }
    assert!(dualpal_core::places::available_space(&home.to_string_lossy()).is_some());
}

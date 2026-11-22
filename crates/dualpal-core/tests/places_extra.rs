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

/// PLC-004 — reordering out of range errors and changes nothing.
#[test]
fn reorder_out_of_range_errors() {
    let favs = Favorites::in_memory(vec![fav("A", "/a"), fav("B", "/b")]);
    assert!(favs.reorder(5, 0).is_err());
    assert_eq!(
        favs.reorder(0, 2).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    let names: Vec<_> = favs.list().into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["A", "B"]);
    // Moving to the last position is in range.
    let moved = favs.reorder(0, 1).unwrap();
    assert_eq!(moved[1].name, "A");
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

/// PLACES-FAV — two copies of the app changing the same favorites file
/// keep each other's changes: each change reads the file again under a lock.
#[test]
fn favorites_changed_by_two_copies_keep_both_changes() {
    use dualpal_core::places::{Favorite, Favorites};
    use dualpal_core::Location;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("favorites.json");
    std::fs::write(&path, "[]").unwrap();
    let fav = |name: &str| Favorite {
        name: name.into(),
        location: Location::new("local", format!("/{name}")),
        icon: "folder".into(),
    };
    let one = Favorites::open(&path);
    let two = Favorites::open(&path);
    one.add(fav("a")).unwrap();
    let seen = two.add(fav("b")).unwrap();
    let names: Vec<_> = seen.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["a", "b"]);
    let on_disk: Vec<Favorite> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk.len(), 2);
    // A reorder works on the list as it is on disk, too.
    let moved = one.reorder(1, 0).unwrap();
    assert_eq!(
        moved.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
}

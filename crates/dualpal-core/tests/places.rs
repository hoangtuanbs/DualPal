use dualpal_core::places::{self, Favorite, Favorites};
use dualpal_core::Location;

fn fav(name: &str) -> Favorite {
    Favorite {
        name: name.into(),
        location: Location::new("local", format!("/{name}")),
        icon: "folder".into(),
    }
}

#[test]
fn favorites_persist_add_remove_reorder() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("favorites.json");
    std::fs::write(&path, "[]").unwrap();
    let favs = Favorites::open(&path);
    favs.add(fav("a")).unwrap();
    favs.add(fav("b")).unwrap();
    favs.add(fav("c")).unwrap();
    favs.add(fav("a")).unwrap(); // duplicate ignored
    favs.reorder(2, 0).unwrap();
    favs.remove(&Location::new("local", "/b")).unwrap();

    let names = |f: &Favorites| f.list().into_iter().map(|f| f.name).collect::<Vec<_>>();
    assert_eq!(names(&favs), ["c", "a"]);
    assert_eq!(names(&Favorites::open(&path)), ["c", "a"]);
}

#[test]
fn missing_favorites_file_starts_with_standard_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let favs = Favorites::open(tmp.path().join("none.json"));
    assert_eq!(favs.list(), places::standard_folders());
}

#[test]
fn volumes_include_the_system_disk() {
    let vols = places::volumes();
    assert!(!vols.is_empty(), "at least one volume");
    let home = dirs::home_dir().unwrap();
    assert!(places::available_space(&home.to_string_lossy()).is_some());
}

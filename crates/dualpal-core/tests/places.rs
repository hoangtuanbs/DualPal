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

#[test]
fn wsl_names_hide_docker_and_keep_spaces() {
    use dualpal_core::places::{wsl_names, wsl_volumes};
    let names = wsl_names(vec![
        "Ubuntu-22.04".into(),
        "docker-desktop".into(),
        "docker-desktop-data".into(),
        "My  Distro".into(),
        "".into(),
        "Ubuntu-22.04".into(),
    ]);
    assert_eq!(names, vec!["Ubuntu-22.04", "My  Distro"]);
    let vols = wsl_volumes(&names);
    assert_eq!(vols[0].location.path, r"\\wsl.localhost\Ubuntu-22.04\");
    assert_eq!(vols[1].location.path, r"\\wsl.localhost\My  Distro\");
    assert!(vols.iter().all(|v| v.wsl));
}

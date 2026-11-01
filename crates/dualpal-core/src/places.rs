//! Places for the sidebar: mounted volumes, standard folders and the
//! user's own favorites (persisted as JSON).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::vfs::{LocalFs, Location};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub location: Location,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    /// A WSL distribution, opened through `\\wsl.localhost\<name>\`.
    #[serde(default)]
    pub wsl: bool,
}

/// Mounted volumes the user cares about (system-internal mounts are hidden).
pub fn volumes() -> Vec<Volume> {
    let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() else {
        return Vec::new();
    };
    let mut out: Vec<Volume> = disks
        .iter()
        .filter(|d| is_user_visible(d.mount_point()))
        .map(|d| {
            let mount = d.mount_point();
            Volume {
                name: volume_name(mount, &d.name().to_string_lossy()),
                location: Location::new(LocalFs::ID, mount.to_string_lossy()),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                removable: d.is_removable(),
                wsl: false,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.location.path != "/", &a.location.path).cmp(&(b.location.path != "/", &b.location.path))
    });
    out.dedup_by(|a, b| a.location == b.location);
    out.extend(wsl_distros());
    out
}

/// WSL distributions installed for this user (Windows only). Read from the
/// registry so that listing them does not start their virtual machines.
pub fn wsl_distros() -> Vec<Volume> {
    #[cfg(windows)]
    {
        wsl_volumes(&wsl_names(registry::wsl_distribution_names()))
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// The distributions to show: without Docker Desktop's internal ones,
/// empty names or duplicates.
pub fn wsl_names(mut names: Vec<String>) -> Vec<String> {
    names.retain(|n| !n.is_empty() && !n.to_ascii_lowercase().starts_with("docker-desktop"));
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    names
}

#[cfg(windows)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// `DistributionName` of every subkey of HKCU\...\Lxss, read as UTF-16.
    pub fn wsl_distribution_names() -> Vec<String> {
        let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
        let value = wide("DistributionName");
        let mut names = Vec::new();
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key)
                != ERROR_SUCCESS
            {
                return names;
            }
            for index in 0.. {
                let mut sub = [0u16; 256];
                let mut sub_len = sub.len() as u32;
                let status = RegEnumKeyExW(
                    key,
                    index,
                    sub.as_mut_ptr(),
                    &mut sub_len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if status != ERROR_SUCCESS {
                    break;
                }
                let mut data = [0u16; 512];
                let mut bytes = (data.len() * 2) as u32;
                if RegGetValueW(
                    key,
                    sub.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    data.as_mut_ptr().cast(),
                    &mut bytes,
                ) == ERROR_SUCCESS
                {
                    let len = data.iter().position(|&c| c == 0).unwrap_or(data.len());
                    names.push(String::from_utf16_lossy(&data[..len]));
                }
            }
            RegCloseKey(key);
        }
        names
    }
}

/// Sidebar entries for WSL distributions, opening at the distribution's root.
pub fn wsl_volumes(names: &[String]) -> Vec<Volume> {
    names
        .iter()
        .map(|name| Volume {
            name: name.clone(),
            location: Location::new(LocalFs::ID, format!(r"\\wsl.localhost\{name}\")),
            total_bytes: 0,
            available_bytes: 0,
            removable: false,
            wsl: true,
        })
        .collect()
}

fn is_user_visible(mount: &Path) -> bool {
    let p = mount.to_string_lossy();
    if cfg!(target_os = "macos") {
        p == "/" || (p.starts_with("/Volumes/") && !p.starts_with("/Volumes/com.apple"))
    } else if cfg!(windows) {
        true
    } else {
        p == "/"
            || p.starts_with("/media/")
            || p.starts_with("/mnt/")
            || p.starts_with("/run/media/")
            || p == "/home"
    }
}

fn volume_name(mount: &Path, disk_name: &str) -> String {
    if cfg!(target_os = "macos") && mount == Path::new("/") {
        return "Macintosh HD".into();
    }
    if cfg!(windows) {
        let letter = mount.to_string_lossy().trim_end_matches('\\').to_string();
        return if disk_name.is_empty() {
            format!("Local Disk ({letter})")
        } else {
            format!("{disk_name} ({letter})")
        };
    }
    if mount == Path::new("/") {
        return "Computer".into();
    }
    mount
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| disk_name.to_string())
}

/// Free space of the volume holding `path` (longest matching mount point).
pub fn available_space(path: &str) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|d| Path::new(path).starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub name: String,
    pub location: Location,
    /// Icon hint for the UI: "home", "desktop", "documents", "downloads",
    /// "pictures", "music", "movies", "applications" or "folder".
    pub icon: String,
}

/// Standard folders that exist on this machine, used as the default favorites.
pub fn standard_folders() -> Vec<Favorite> {
    let mut v: Vec<(String, Option<PathBuf>, &str)> = vec![
        ("Home".into(), dirs::home_dir(), "home"),
        ("Desktop".into(), dirs::desktop_dir(), "desktop"),
        ("Documents".into(), dirs::document_dir(), "documents"),
        ("Downloads".into(), dirs::download_dir(), "downloads"),
        ("Pictures".into(), dirs::picture_dir(), "pictures"),
        ("Music".into(), dirs::audio_dir(), "music"),
        (
            if cfg!(target_os = "macos") {
                "Movies"
            } else {
                "Videos"
            }
            .into(),
            dirs::video_dir(),
            "movies",
        ),
    ];
    if let Some(home) = dirs::home_dir() {
        if let Some(name) = home.file_name() {
            v[0].0 = name.to_string_lossy().into_owned();
        }
    }
    if cfg!(target_os = "macos") {
        v.push((
            "Applications".into(),
            Some(PathBuf::from("/Applications")),
            "applications",
        ));
    }
    v.into_iter()
        .filter_map(|(name, path, icon)| {
            let path = path.filter(|p| p.is_dir())?;
            Some(Favorite {
                name,
                location: Location::new(LocalFs::ID, path.to_string_lossy()),
                icon: icon.into(),
            })
        })
        .collect()
}

/// The user's favorites, persisted to a JSON file. Starts with the standard
/// folders when no file exists yet.
pub struct Favorites {
    items: Mutex<Vec<Favorite>>,
    file: Option<PathBuf>,
}

impl Favorites {
    pub fn in_memory(items: Vec<Favorite>) -> Self {
        Self {
            items: Mutex::new(items),
            file: None,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(standard_folders);
        Self {
            items: Mutex::new(items),
            file: Some(path),
        }
    }

    pub fn list(&self) -> Vec<Favorite> {
        self.items.lock().unwrap().clone()
    }

    /// Add a favorite (ignored if that location is already a favorite).
    pub fn add(&self, fav: Favorite) -> std::io::Result<Vec<Favorite>> {
        let mut items = self.items.lock().unwrap();
        if !items.iter().any(|f| f.location == fav.location) {
            items.push(fav);
        }
        self.save(&items)?;
        Ok(items.clone())
    }

    pub fn remove(&self, location: &Location) -> std::io::Result<Vec<Favorite>> {
        let mut items = self.items.lock().unwrap();
        items.retain(|f| &f.location != location);
        self.save(&items)?;
        Ok(items.clone())
    }

    /// Reorder: move the favorite at `from` to index `to`.
    pub fn reorder(&self, from: usize, to: usize) -> std::io::Result<Vec<Favorite>> {
        let mut items = self.items.lock().unwrap();
        if from < items.len() {
            let f = items.remove(from);
            let to = to.min(items.len());
            items.insert(to, f);
        }
        self.save(&items)?;
        Ok(items.clone())
    }

    fn save(&self, items: &[Favorite]) -> std::io::Result<()> {
        let Some(path) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(items).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

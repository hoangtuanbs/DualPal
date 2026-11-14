//! Upgrade from a GitHub release by running its installer.
//!
//! Used when a release has no signed updater files (see `updates.svelte.ts`).
//! `latest_release` names the installer that fits this copy, `download_upgrade`
//! fetches and verifies it, and `apply_upgrade` runs it:
//!
//! * Windows: the `-setup.exe` (passive, restarts the app) or the `.msi`
//!   (passive; the app starts again when it is done). The app then quits.
//! * macOS: the app bundle is replaced with the one in the `.dmg`, then the
//!   app restarts. If the bundle cannot be replaced, the `.dmg` is opened.
//! * Linux: an `.AppImage` is replaced in place; a `.deb` or `.rpm` is
//!   installed with `pkexec`, which asks for the password. Without `pkexec`
//!   the package is opened in the system's software installer.
//!
//! `DUALPAL_INSTALL_KIND` and `DUALPAL_UPGRADE_LOG` let tests pick the kind
//! and record the install instead of running it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dualpal_core::releases::{self, ReleaseInfo};
use dualpal_core::upgrade::{self, InstallKind};
use dualpal_core::Error;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

type CmdResult<T> = Result<T, Error>;

/// A downloaded installer waiting for `apply_upgrade`.
struct Prepared {
    kind: InstallKind,
    file: PathBuf,
}

static PREPARED: Mutex<Option<Prepared>> = Mutex::new(None);

/// The newest release, and the installer `download_upgrade` would use for it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestRelease {
    #[serde(flatten)]
    release: ReleaseInfo,
    /// File name of the installer for this copy; empty when the app cannot
    /// install this release itself (unknown install, no matching file).
    installer: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// What the app does after `apply_upgrade`.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// The new version is in place: restart into it.
    Restart,
    /// An installer is running and replaces the app: quit now.
    Exit,
    /// The installer was opened for the person to finish.
    Opened,
}

fn releases_url() -> String {
    std::env::var("DUALPAL_RELEASES_URL").unwrap_or_else(|_| releases::LATEST_RELEASE_URL.into())
}

fn user_agent<R: Runtime>(app: &AppHandle<R>) -> String {
    format!("DualPal/{}", app.package_info().version)
}

/// How this copy was installed, or None when the app cannot tell.
fn install_kind() -> Option<InstallKind> {
    if let Ok(k) = std::env::var("DUALPAL_INSTALL_KIND") {
        return serde_json::from_value(serde_json::Value::String(k)).ok();
    }
    detect_install_kind()
}

#[cfg(target_os = "windows")]
fn detect_install_kind() -> Option<InstallKind> {
    let exe = std::env::current_exe().ok()?;
    let lower = exe.to_string_lossy().to_lowercase();
    // The setup.exe installs per user under %LOCALAPPDATA%; the MSI under Program Files.
    Some(if lower.contains("\\appdata\\local\\") {
        InstallKind::WindowsSetup
    } else {
        InstallKind::WindowsMsi
    })
}

#[cfg(target_os = "macos")]
fn detect_install_kind() -> Option<InstallKind> {
    mac::bundle_path().map(|_| InstallKind::MacApp)
}

#[cfg(target_os = "linux")]
fn detect_install_kind() -> Option<InstallKind> {
    if std::env::var_os("APPIMAGE").is_some() {
        return Some(InstallKind::AppImage);
    }
    let exe = std::env::current_exe().ok()?;
    let owned_by = |tool: &str, args: &[&str]| {
        std::process::Command::new(tool)
            .args(args)
            .arg(&exe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if owned_by("dpkg", &["-S"]) {
        Some(InstallKind::Deb)
    } else if owned_by("rpm", &["-qf"]) {
        Some(InstallKind::Rpm)
    } else {
        None
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn detect_install_kind() -> Option<InstallKind> {
    None
}

fn installer_for(release: &ReleaseInfo) -> Option<(InstallKind, &releases::ReleaseAsset)> {
    let kind = install_kind()?;
    let asset = upgrade::pick_installer(&release.assets, kind, std::env::consts::ARCH)?;
    Some((kind, asset))
}

/// The newest published release on GitHub, for the upgrade notice.
/// `DUALPAL_RELEASES_URL` points it elsewhere in tests.
#[tauri::command]
pub async fn latest_release<R: Runtime>(app: AppHandle<R>) -> CmdResult<LatestRelease> {
    let release = releases::latest_release(&releases_url(), &user_agent(&app)).await?;
    let installer = installer_for(&release).map(|(_, a)| a.name.clone());
    Ok(LatestRelease { release, installer })
}

/// Download and verify the installer of release `version`, sending
/// `upgrade-progress` events. The release is read again from GitHub, so only
/// its own files are ever fetched.
#[tauri::command]
pub async fn download_upgrade<R: Runtime>(app: AppHandle<R>, version: String) -> CmdResult<String> {
    let release = releases::latest_release(&releases_url(), &user_agent(&app)).await?;
    if release.version != version {
        return Err(Error::Invalid(format!(
            "the newest release is now {}, not {version}; check for updates again",
            release.version
        )));
    }
    let (kind, asset) = installer_for(&release).ok_or_else(|| {
        Error::Unsupported(
            "installing this release from the app; download it from the release page".into(),
        )
    })?;
    let file = download_path(kind, &asset.name, &version)?;
    let handle = app.clone();
    upgrade::download_verified(asset, &file, &user_agent(&app), move |downloaded, total| {
        let _ = handle.emit("upgrade-progress", Progress { downloaded, total });
    })
    .await?;
    *PREPARED.lock().unwrap() = Some(Prepared { kind, file });
    Ok(asset.name.clone())
}

/// Where the installer goes: next to the AppImage it replaces (same disk, so
/// the swap is a rename), else a folder in the temp directory.
fn download_path(kind: InstallKind, name: &str, version: &str) -> CmdResult<PathBuf> {
    if kind == InstallKind::AppImage {
        if let Some(target) = std::env::var_os("APPIMAGE") {
            let mut p = PathBuf::from(target).into_os_string();
            p.push(".download");
            return Ok(PathBuf::from(p));
        }
    }
    let dir = std::env::temp_dir().join(format!("dualpal-upgrade-{version}"));
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
    // Keep the file name: msiexec and Finder show it.
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(dir.join(safe))
}

/// Run the installer `download_upgrade` fetched. The UI then restarts or quits.
#[tauri::command]
pub async fn apply_upgrade() -> CmdResult<Outcome> {
    let prepared = PREPARED
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| Error::Invalid("no update has been downloaded".into()))?;
    let log_line = format!("{:?} {}", prepared.kind, prepared.file.display());
    if dualpal_core::launch_log::record("DUALPAL_UPGRADE_LOG", &log_line) {
        return Ok(match prepared.kind {
            InstallKind::WindowsSetup | InstallKind::WindowsMsi => Outcome::Exit,
            _ => Outcome::Restart,
        });
    }
    tokio::task::spawn_blocking(move || run_installer(prepared.kind, &prepared.file))
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
}

fn spawn_error(what: &str, e: std::io::Error) -> Error {
    Error::Io {
        path: what.to_string(),
        source: e,
    }
}

#[cfg(target_os = "windows")]
fn run_installer(kind: InstallKind, file: &Path) -> CmdResult<Outcome> {
    use std::process::Command;
    match kind {
        InstallKind::WindowsSetup => {
            // /P: passive, progress only. /R: start the app again when done.
            Command::new(file)
                .args(["/P", "/R"])
                .spawn()
                .map_err(|e| spawn_error(&file.to_string_lossy(), e))?;
        }
        InstallKind::WindowsMsi => {
            let exe = std::env::current_exe().map_err(|e| spawn_error("current_exe", e))?;
            let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
            // Wait for msiexec, then start the new version.
            let script = format!(
                "$p = Start-Process msiexec -ArgumentList '/i',{},'/passive' -Wait -PassThru; if ($p.ExitCode -eq 0 -or $p.ExitCode -eq 3010) {{ Start-Process {} }}",
                quote(file),
                quote(&exe)
            );
            Command::new("powershell")
                .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
                .spawn()
                .map_err(|e| spawn_error("powershell", e))?;
        }
        _ => return Err(Error::Unsupported(format!("{kind:?} on Windows"))),
    }
    Ok(Outcome::Exit)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{spawn_error, CmdResult, Outcome};
    use dualpal_core::Error;

    /// The `.app` bundle this process runs from.
    pub fn bundle_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        exe.ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .map(Path::to_path_buf)
    }

    fn run(cmd: &mut Command, what: &str) -> CmdResult<()> {
        let status = cmd.status().map_err(|e| spawn_error(what, e))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Invalid(format!("{what} failed ({status})")))
        }
    }

    /// Copy the app out of the disk image over this one.
    fn replace(dmg: &Path, mount: &Path) -> CmdResult<()> {
        let bundle = bundle_path()
            .ok_or_else(|| Error::Unsupported("running outside an app bundle".into()))?;
        let new_app = std::fs::read_dir(mount)
            .map_err(|e| Error::io(mount.to_string_lossy(), e))?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| Error::NotFound(format!("an app in {}", dmg.display())))?;
        let parent = bundle
            .parent()
            .ok_or_else(|| Error::Invalid("app folder".into()))?;
        let staged = parent.join(".Dual Pal upgrade.app");
        let old = parent.join(".Dual Pal previous.app");
        let _ = std::fs::remove_dir_all(&staged);
        let _ = std::fs::remove_dir_all(&old);
        run(Command::new("ditto").arg(&new_app).arg(&staged), "ditto")?;
        std::fs::rename(&bundle, &old).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            Error::io(bundle.to_string_lossy(), e)
        })?;
        if let Err(e) = std::fs::rename(&staged, &bundle) {
            let _ = std::fs::rename(&old, &bundle);
            let _ = std::fs::remove_dir_all(&staged);
            return Err(Error::io(bundle.to_string_lossy(), e));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    }

    pub fn install(dmg: &Path) -> CmdResult<Outcome> {
        let mount = dmg.with_extension("mount");
        let _ = std::fs::create_dir_all(&mount);
        run(
            Command::new("hdiutil")
                .args([
                    "attach",
                    "-nobrowse",
                    "-readonly",
                    "-noautoopen",
                    "-mountpoint",
                ])
                .arg(&mount)
                .arg(dmg),
            "hdiutil attach",
        )?;
        let replaced = replace(dmg, &mount);
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet"])
            .arg(&mount)
            .status();
        match replaced {
            Ok(()) => Ok(Outcome::Restart),
            Err(e) => {
                // No write access to the app's folder, for example: let Finder do it.
                eprintln!("dualpal: replacing the app failed ({e}); opening the disk image");
                run(Command::new("open").arg(dmg), "open")?;
                Ok(Outcome::Opened)
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn run_installer(kind: InstallKind, file: &Path) -> CmdResult<Outcome> {
    match kind {
        InstallKind::MacApp => mac::install(file),
        _ => Err(Error::Unsupported(format!("{kind:?} on macOS"))),
    }
}

#[cfg(target_os = "linux")]
fn run_installer(kind: InstallKind, file: &Path) -> CmdResult<Outcome> {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    match kind {
        InstallKind::AppImage => {
            let target = std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .ok_or_else(|| Error::Unsupported("AppImage path unknown".into()))?;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| Error::io(file.to_string_lossy(), e))?;
            std::fs::rename(file, &target).map_err(|e| Error::io(target.to_string_lossy(), e))?;
            Ok(Outcome::Restart)
        }
        InstallKind::Deb | InstallKind::Rpm => {
            let (tool, args): (&str, &[&str]) = if kind == InstallKind::Deb {
                ("dpkg", &["-i"])
            } else {
                ("rpm", &["-U"])
            };
            match Command::new("pkexec")
                .arg(tool)
                .args(args)
                .arg(file)
                .status()
            {
                Ok(s) if s.success() => Ok(Outcome::Restart),
                // 126: the password prompt was dismissed; 127: not authorized.
                Ok(s) if matches!(s.code(), Some(126) | Some(127)) => Err(Error::Cancelled),
                Ok(s) => Err(Error::Invalid(format!(
                    "installing the package failed ({s})"
                ))),
                Err(_) => {
                    // No pkexec: hand the package to the software installer.
                    Command::new("xdg-open")
                        .arg(file)
                        .spawn()
                        .map_err(|e| spawn_error("xdg-open", e))?;
                    Ok(Outcome::Opened)
                }
            }
        }
        _ => Err(Error::Unsupported(format!("{kind:?} on Linux"))),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn run_installer(kind: InstallKind, _file: &Path) -> CmdResult<Outcome> {
    Err(Error::Unsupported(format!("{kind:?} here")))
}

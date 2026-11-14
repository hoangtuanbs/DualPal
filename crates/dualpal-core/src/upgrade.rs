//! Upgrading Dual Pal from a GitHub release without the Tauri updater.
//!
//! When a release has no signed updater files, the app downloads the
//! installer that matches how it was installed, checks it against GitHub's
//! SHA-256 digest and runs it. This module picks the file and downloads it;
//! running it is platform work in the app crate.

use std::path::Path;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::releases::ReleaseAsset;

/// How this copy of Dual Pal was installed, which decides the file to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallKind {
    /// Windows, installed per user with the `-setup.exe` (NSIS) installer.
    WindowsSetup,
    /// Windows, installed with the `.msi` installer.
    WindowsMsi,
    /// macOS app bundle, upgraded from the `.dmg`.
    MacApp,
    /// Linux `.AppImage`, replaced in place.
    AppImage,
    /// Linux, installed from the `.deb` package.
    Deb,
    /// Linux, installed from the `.rpm` package.
    Rpm,
}

impl InstallKind {
    fn suffix(self) -> &'static str {
        match self {
            InstallKind::WindowsSetup => "-setup.exe",
            InstallKind::WindowsMsi => ".msi",
            InstallKind::MacApp => ".dmg",
            InstallKind::AppImage => ".appimage",
            InstallKind::Deb => ".deb",
            InstallKind::Rpm => ".rpm",
        }
    }
}

/// Names a release uses for a CPU architecture (`std::env::consts::ARCH`).
fn arch_names(arch: &str) -> &'static [&'static str] {
    match arch {
        "x86_64" => &["x64", "amd64", "x86_64"],
        "aarch64" => &["arm64", "aarch64"],
        "x86" => &["x86", "i386", "i686"],
        _ => &[],
    }
}

/// Every architecture name, to tell "built for another CPU" from "no CPU named".
const ALL_ARCH_NAMES: &[&str] = &[
    "x64",
    "amd64",
    "x86_64",
    "arm64",
    "aarch64",
    "x86",
    "i386",
    "i686",
    "universal",
];

/// The tokens of a file name, split on `_`, `-` and `.`.
fn tokens(name: &str) -> Vec<String> {
    // "x86_64" is one name although it holds a separator.
    name.to_ascii_lowercase()
        .replace("x86_64", "amd64")
        .split(['_', '-', '.'])
        .map(str::to_string)
        .collect()
}

/// The installer for `kind` on `arch` among a release's files. A file built
/// for this CPU wins over a universal one; a file built for another CPU is
/// never picked. Updater archives and signatures are left out.
pub fn pick_installer<'a>(
    assets: &'a [ReleaseAsset],
    kind: InstallKind,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let mine = arch_names(arch);
    let mut universal = None;
    for a in assets {
        let lower = a.name.to_ascii_lowercase();
        if !lower.ends_with(kind.suffix()) {
            continue;
        }
        let t = tokens(&a.name);
        if t.iter().any(|x| mine.contains(&x.as_str())) {
            return Some(a);
        }
        let other_cpu = t
            .iter()
            .any(|x| ALL_ARCH_NAMES.contains(&x.as_str()) && x != "universal");
        if !other_cpu && universal.is_none() {
            universal = Some(a);
        }
    }
    universal
}

/// Download `asset` to `dest`, reporting `(downloaded, total)` as it goes,
/// and check GitHub's SHA-256 when the release has one. On any failure the
/// partial file is removed.
pub async fn download_verified(
    asset: &ReleaseAsset,
    dest: &Path,
    user_agent: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    let result = download_inner(asset, dest, user_agent, &mut progress).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(dest).await;
    }
    result
}

async fn download_inner(
    asset: &ReleaseAsset,
    dest: &Path,
    user_agent: &str,
    progress: &mut impl FnMut(u64, Option<u64>),
) -> Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| Error::Connection(e.to_string()))?;
    let resp = client
        .get(&asset.url)
        .header(reqwest::header::USER_AGENT, user_agent)
        .send()
        .await
        .map_err(|e| Error::Connection(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Connection(format!(
            "downloading {} failed: {}",
            asset.name,
            resp.status()
        )));
    }
    let total = resp
        .content_length()
        .or((asset.size > 0).then_some(asset.size));
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| Error::io(dest.to_string_lossy(), e))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    progress(0, total);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Connection(e.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| Error::io(dest.to_string_lossy(), e))?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush()
        .await
        .map_err(|e| Error::io(dest.to_string_lossy(), e))?;
    drop(file);
    if let Some(expected) = &asset.sha256 {
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &actual != expected {
            return Err(Error::Invalid(format!(
                "{} does not match the checksum GitHub published; it was not installed",
                asset.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.into(),
            url: format!("https://github.com/x/y/releases/download/v0.3.0/{name}"),
            size: 1,
            sha256: None,
        }
    }

    fn release() -> Vec<ReleaseAsset> {
        [
            "Dual.Pal-0.3.0-1.x86_64.rpm",
            "Dual.Pal_0.3.0_amd64.AppImage",
            "Dual.Pal_0.3.0_amd64.AppImage.sig",
            "Dual.Pal_0.3.0_amd64.deb",
            "Dual.Pal_0.3.0_universal.dmg",
            "Dual.Pal.app.tar.gz",
            "Dual.Pal_0.3.0_x64-setup.exe",
            "Dual.Pal_0.3.0_x64-setup.exe.sig",
            "Dual.Pal_0.3.0_x64_en-US.msi",
            "latest.json",
        ]
        .into_iter()
        .map(asset)
        .collect()
    }

    fn pick(kind: InstallKind, arch: &str) -> Option<String> {
        pick_installer(&release(), kind, arch).map(|a| a.name.clone())
    }

    #[test]
    fn picks_the_installer_for_each_kind_on_x86_64() {
        use InstallKind::*;
        let x = "x86_64";
        assert_eq!(
            pick(WindowsSetup, x).unwrap(),
            "Dual.Pal_0.3.0_x64-setup.exe"
        );
        assert_eq!(pick(WindowsMsi, x).unwrap(), "Dual.Pal_0.3.0_x64_en-US.msi");
        assert_eq!(pick(MacApp, x).unwrap(), "Dual.Pal_0.3.0_universal.dmg");
        assert_eq!(pick(AppImage, x).unwrap(), "Dual.Pal_0.3.0_amd64.AppImage");
        assert_eq!(pick(Deb, x).unwrap(), "Dual.Pal_0.3.0_amd64.deb");
        assert_eq!(pick(Rpm, x).unwrap(), "Dual.Pal-0.3.0-1.x86_64.rpm");
    }

    #[test]
    fn never_picks_a_file_for_another_cpu() {
        assert_eq!(
            pick(InstallKind::MacApp, "aarch64").unwrap(),
            "Dual.Pal_0.3.0_universal.dmg"
        );
        assert_eq!(pick(InstallKind::Deb, "aarch64"), None);
        assert_eq!(pick(InstallKind::WindowsSetup, "aarch64"), None);
        let mut assets = release();
        assets.push(asset("Dual.Pal_0.3.0_arm64-setup.exe"));
        assert_eq!(
            pick_installer(&assets, InstallKind::WindowsSetup, "aarch64")
                .unwrap()
                .name,
            "Dual.Pal_0.3.0_arm64-setup.exe"
        );
    }

    #[test]
    fn prefers_this_cpu_over_universal() {
        let assets = vec![
            asset("Dual.Pal_0.3.0_universal.dmg"),
            asset("Dual.Pal_0.3.0_aarch64.dmg"),
        ];
        assert_eq!(
            pick_installer(&assets, InstallKind::MacApp, "aarch64")
                .unwrap()
                .name,
            "Dual.Pal_0.3.0_aarch64.dmg"
        );
    }

    async fn serve_once(body: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body).await.unwrap();
        });
        format!("http://127.0.0.1:{port}/file")
    }

    const BODY: &[u8] = b"installer bytes";
    // sha256("installer bytes")
    fn body_sha() -> String {
        Sha256::digest(BODY)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[tokio::test]
    async fn downloads_with_progress_and_checks_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("setup.exe");
        let a = ReleaseAsset {
            name: "setup.exe".into(),
            url: serve_once(BODY).await,
            size: BODY.len() as u64,
            sha256: Some(body_sha()),
        };
        let mut seen = Vec::new();
        download_verified(&a, &dest, "test", |d, t| seen.push((d, t)))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert_eq!(seen.first(), Some(&(0, Some(BODY.len() as u64))));
        assert_eq!(
            seen.last(),
            Some(&(BODY.len() as u64, Some(BODY.len() as u64)))
        );
    }

    #[tokio::test]
    async fn refuses_and_removes_a_file_with_the_wrong_digest() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("setup.exe");
        let a = ReleaseAsset {
            name: "setup.exe".into(),
            url: serve_once(BODY).await,
            size: 0,
            sha256: Some("0".repeat(64)),
        };
        let err = download_verified(&a, &dest, "test", |_, _| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        assert!(!dest.exists());
    }
}

//! WSL folders seen from Windows through `\\wsl.localhost\<distro>\` (or the
//! older `\\wsl$\<distro>\`).
//!
//! Windows can tell that an entry there is a Linux symlink but cannot read
//! or follow it, so `bin -> usr/bin` would look like an empty file. These
//! helpers ask the distribution itself, through `wsl.exe`.

/// A path inside a WSL distribution, split into the UNC share
/// (`\\wsl.localhost\Ubuntu`), the distribution name and the Linux path.
#[derive(Debug, PartialEq, Eq)]
pub struct WslPath {
    pub share: String,
    pub distro: String,
    pub linux: String,
}

pub fn split(path: &str) -> Option<WslPath> {
    let rest = path
        .strip_prefix(r"\\")
        .or_else(|| path.strip_prefix("//"))?;
    let mut parts = rest.splitn(3, ['\\', '/']);
    let host = parts.next()?;
    if !(host.eq_ignore_ascii_case("wsl.localhost") || host.eq_ignore_ascii_case("wsl$")) {
        return None;
    }
    let distro = parts.next().filter(|d| !d.is_empty())?;
    let tail = parts.next().unwrap_or("").replace('\\', "/");
    Some(WslPath {
        share: format!(r"\\{host}\{distro}"),
        distro: distro.to_string(),
        linux: format!("/{}", tail.trim_matches('/')),
    })
}

/// The Windows path of `linux` (an absolute Linux path) in `share`.
pub fn to_windows(share: &str, linux: &str) -> String {
    format!(
        r"{share}\{}",
        linux.trim_start_matches('/').replace('/', "\\")
    )
}

/// Run a command in the distribution and return its standard output.
#[cfg(windows)]
async fn exec(distro: &str, args: &[&str]) -> Option<String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = tokio::process::Command::new("wsl.exe")
        .arg("-d")
        .arg(distro)
        .arg("--exec")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(not(windows))]
async fn exec(_distro: &str, _args: &[&str]) -> Option<String> {
    None
}

/// Where the folder link `path` points, as a Windows path, or None.
pub async fn resolve_dir(path: &str) -> Option<String> {
    let p = split(path)?;
    let out = exec(&p.distro, &["readlink", "-f", "--", &p.linux]).await?;
    let target = out.lines().next()?.trim();
    target
        .starts_with('/')
        .then(|| to_windows(&p.share, target))
}

/// Which of `paths` (links in one distribution) point to folders. One
/// `wsl.exe` call per batch, kept under Windows' command line limit.
pub async fn which_are_dirs(paths: &[String]) -> Vec<bool> {
    let mut out = vec![false; paths.len()];
    let Some(first) = paths.first().and_then(|p| split(p)) else {
        return out;
    };
    let linux: Vec<String> = paths
        .iter()
        .map(|p| split(p).map(|w| w.linux).unwrap_or_default())
        .collect();
    let mut start = 0;
    while start < linux.len() {
        let mut end = start;
        let mut len = 0;
        while end < linux.len() && (end == start || len + linux[end].len() < 20_000) {
            len += linux[end].len() + 3;
            end += 1;
        }
        let mut args = vec![
            "sh",
            "-c",
            r#"for p; do if [ -d "$p" ]; then echo d; else echo -; fi; done"#,
            "sh",
        ];
        args.extend(linux[start..end].iter().map(String::as_str));
        if let Some(text) = exec(&first.distro, &args).await {
            for (i, line) in text.lines().take(end - start).enumerate() {
                out[start + i] = line.trim() == "d";
            }
        }
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_wsl_paths() {
        let p = split(r"\\wsl.localhost\Ubuntu-22.04\usr\lib").unwrap();
        assert_eq!(p.share, r"\\wsl.localhost\Ubuntu-22.04");
        assert_eq!(p.distro, "Ubuntu-22.04");
        assert_eq!(p.linux, "/usr/lib");
        assert_eq!(split(r"\\wsl$\Debian\").unwrap().linux, "/");
        assert_eq!(split(r"\\WSL.LOCALHOST\Debian").unwrap().linux, "/");
        assert!(split(r"\\server\share\x").is_none());
        assert!(split(r"C:\Users").is_none());
        assert!(split(r"\\wsl.localhost\").is_none());
    }

    #[test]
    fn maps_linux_paths_back() {
        assert_eq!(
            to_windows(r"\\wsl.localhost\Ubuntu", "/usr/bin"),
            r"\\wsl.localhost\Ubuntu\usr\bin"
        );
        assert_eq!(to_windows(r"\\wsl$\Ubuntu", "/"), r"\\wsl$\Ubuntu\");
    }
}

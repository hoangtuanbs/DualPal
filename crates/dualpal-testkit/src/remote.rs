//! Helpers for the remote (L3) tests: fail when a server is missing if the
//! suite is required to run, build backends from the `DUALPAL_TEST_*`
//! variables, and hand out a fresh folder per test.

use std::sync::Arc;

use dualpal_core::connections::{self, ConnectOptions, Connection, Endpoint, SshAuth};
use dualpal_core::vfs::{FtpSecurity, LocalFs};
use dualpal_core::{Error, FileSystem, Location};
use tempfile::TempDir;

/// Read a `DUALPAL_TEST_*` variable. With `DUALPAL_REQUIRE_REMOTE=1` a
/// missing value is a test failure, not a silent skip (PREP-07).
pub fn remote_env(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            if std::env::var("DUALPAL_REQUIRE_REMOTE").as_deref() == Ok("1") {
                panic!("{var} must be set when DUALPAL_REQUIRE_REMOTE=1");
            }
            eprintln!("skipped: {var} is not set");
            None
        }
    }
}

/// A connected backend plus whatever temp files its trust flow needed.
pub struct TestFs {
    pub fs: Arc<dyn FileSystem>,
    pub start_path: String,
    /// Kept alive so a later reconnect can read the same known_hosts file.
    pub _temp: Option<TempDir>,
}

/// `DUALPAL_TEST_SFTP=user@host:port` + `DUALPAL_TEST_SFTP_KEY=/path/key`.
pub async fn sftp_from_env() -> Option<TestFs> {
    let target = remote_env("DUALPAL_TEST_SFTP")?;
    let key = remote_env("DUALPAL_TEST_SFTP_KEY")?;
    let (user, rest) = target.split_once('@')?;
    let (host, port) = rest.rsplit_once(':')?;
    let temp = tempfile::tempdir().ok()?;
    let known_hosts = temp.path().join("known_hosts");
    let conn = Connection {
        id: "test-sftp".into(),
        name: "Test SFTP".into(),
        endpoint: Endpoint::Sftp {
            host: host.into(),
            port: port.parse().ok()?,
            user: user.into(),
            auth: SshAuth::KeyFile { path: key },
        },
        initial_path: None,
        favorites: vec![],
    };
    let opts = |trust: Option<String>| ConnectOptions {
        secret: None,
        trust_fingerprint: trust,
        known_hosts: Some(known_hosts.clone()),
        known_certs: None,
    };
    let connected = match connections::connect(&conn, None, opts(None)).await {
        Ok(c) => c,
        Err(Error::UnknownHost { fingerprint, .. }) => {
            connections::connect(&conn, None, opts(Some(fingerprint)))
                .await
                .expect("trusting the server's own fingerprint")
        }
        Err(e) => panic!("connecting to {target}: {e}"),
    };
    Some(TestFs {
        fs: connected.fs,
        start_path: connected.start_path,
        _temp: Some(temp),
    })
}

/// `DUALPAL_TEST_FTP`, `_FTPS` or `_FTPS_IMPLICIT`, e.g.
/// `user:pass@127.0.0.1:2121`.
pub async fn ftp_from_env(var: &str, security: FtpSecurity) -> Option<TestFs> {
    let spec = remote_env(var)?;
    let (userinfo, hostport) = spec.rsplit_once('@')?;
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let (host, port) = hostport.rsplit_once(':')?;
    let temp = tempfile::tempdir().ok()?;
    let conn = Connection {
        id: format!("test-{var}"),
        name: var.into(),
        endpoint: Endpoint::Ftp {
            host: host.into(),
            port: port.parse().ok()?,
            user: user.into(),
            security,
        },
        initial_path: None,
        favorites: vec![],
    };
    let opts = |trust: Option<String>| ConnectOptions {
        trust_fingerprint: trust,
        known_certs: Some(temp.path().join("known_certificates.json")),
        ..Default::default()
    };
    // FTPS servers in the test setup use a self-signed certificate: trust the
    // fingerprint the first attempt reports, as the app's dialog would.
    let connected = match connections::connect(&conn, Some(password.to_string()), opts(None)).await
    {
        Ok(c) => c,
        Err(Error::UnknownHost { fingerprint, .. }) => {
            connections::connect(&conn, Some(password.to_string()), opts(Some(fingerprint)))
                .await
                .unwrap_or_else(|e| panic!("connecting to {spec} after trusting it: {e}"))
        }
        Err(e) => panic!("connecting to {spec}: {e}"),
    };
    Some(TestFs {
        fs: connected.fs,
        start_path: connected.start_path,
        _temp: Some(temp),
    })
}

/// `DUALPAL_TEST_WEBDAV=user:pass@http://127.0.0.1:8088/dav`.
pub async fn webdav_from_env() -> Option<TestFs> {
    let spec = remote_env("DUALPAL_TEST_WEBDAV")?;
    let (userinfo, url) = spec.rsplit_once('@')?;
    let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
    let conn = Connection {
        id: "test-webdav".into(),
        name: "Test WebDAV".into(),
        endpoint: Endpoint::WebDav {
            url: url.into(),
            user: user.into(),
        },
        initial_path: None,
        favorites: vec![],
    };
    let connected =
        connections::connect(&conn, Some(password.to_string()), ConnectOptions::default())
            .await
            .unwrap_or_else(|e| panic!("connecting to {spec}: {e}"));
    Some(TestFs {
        fs: connected.fs,
        start_path: connected.start_path,
        _temp: None,
    })
}

/// `DUALPAL_TEST_S3=endpoint|bucket|access|secret`.
pub async fn s3_from_env() -> Option<TestFs> {
    let spec = remote_env("DUALPAL_TEST_S3")?;
    let parts: Vec<&str> = spec.split('|').collect();
    assert_eq!(
        parts.len(),
        4,
        "DUALPAL_TEST_S3 must be endpoint|bucket|access|secret"
    );
    let conn = Connection {
        id: "test-s3".into(),
        name: "Test S3".into(),
        endpoint: Endpoint::S3 {
            bucket: parts[1].into(),
            region: "us-east-1".into(),
            endpoint: Some(parts[0].into()),
            access_key_id: parts[2].into(),
            path_style: true,
        },
        initial_path: None,
        favorites: vec![],
    };
    let connected =
        connections::connect(&conn, Some(parts[3].to_string()), ConnectOptions::default())
            .await
            .unwrap_or_else(|e| panic!("connecting to {}: {e}", parts[0]));
    Some(TestFs {
        fs: connected.fs,
        start_path: connected.start_path,
        _temp: None,
    })
}

/// A fresh folder on `fs`, removed when the value is dropped.
pub struct UniqueBase {
    fs: Arc<dyn FileSystem>,
    path: String,
}

impl UniqueBase {
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl std::fmt::Display for UniqueBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path)
    }
}

impl Drop for UniqueBase {
    fn drop(&mut self) {
        let fs = self.fs.clone();
        let path = self.path.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = remove_tree(&fs, &path).await;
            });
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(async {
                        let _ = remove_tree(&fs, &path).await;
                    });
                }
            });
        }
    }
}

/// Create a fresh `<root>/dualpal-test/<uuid>` folder on `fs`. `root` is a
/// folder the test user can write to, such as the connection's start path.
pub async fn unique_base(fs: &Arc<dyn FileSystem>, root: &str) -> UniqueBase {
    let parent = fs.join(root, "dualpal-test");
    let path = fs.join(&parent, &uuid::Uuid::new_v4().to_string());
    for dir in [&parent, &path] {
        match fs.create_dir(dir).await {
            Ok(()) | Err(Error::AlreadyExists(_)) => {}
            // Parallel cases share `parent`: another one may have just made it.
            Err(_) if fs.exists(dir).await.unwrap_or(false) => {}
            Err(e) => panic!("creating {dir}: {e}"),
        }
    }
    UniqueBase {
        fs: fs.clone(),
        path,
    }
}

/// The local disk as a test backend.
pub fn local() -> Arc<dyn FileSystem> {
    Arc::new(LocalFs::new())
}

async fn remove_tree(fs: &Arc<dyn FileSystem>, path: &str) {
    let Ok(root) = fs.stat(path).await else {
        return;
    };
    if !root.is_dir() {
        let _ = fs.remove_file(path).await;
        return;
    }
    let mut stack = vec![root];
    let mut items = Vec::new();
    while let Some(e) = stack.pop() {
        if e.is_dir() && !e.symlink {
            if let Ok(children) = fs.list(&e.path).await {
                stack.extend(children);
            }
        }
        let dir = e.is_dir() && !e.symlink;
        items.push((e.path, dir));
    }
    for (p, dir) in items.into_iter().rev() {
        if dir {
            let _ = fs.remove_dir(&p).await;
        } else {
            let _ = fs.remove_file(&p).await;
        }
    }
}

/// A location in `fs` for a path, for convenience in tests.
pub fn at(fs: &Arc<dyn FileSystem>, path: impl Into<String>) -> Location {
    Location::new(fs.id(), path)
}

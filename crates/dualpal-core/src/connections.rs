//! Saved remote connections (SFTP and S3) and the favorites inside them.
//!
//! A connection is itself a favorite in the sidebar, and each one keeps its
//! own list of favorite paths on that server: favorites within favorites.
//! Secrets (passwords, key passphrases, S3 secret keys) are never written to
//! the JSON file; they live in a [`SecretStore`], which the app backs with
//! the macOS Keychain or the Windows Credential Manager.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::vfs::{posix, FileSystem, S3Fs, SftpFs};

/// How to sign in to an SSH server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "method",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SshAuth {
    /// A password (stored as the connection's secret). Also answers
    /// keyboard-interactive password prompts.
    Password,
    /// A private key file. Its passphrase, if any, is the connection's secret.
    KeyFile { path: String },
    /// Keys from the running SSH agent, then the default key files in ~/.ssh.
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Endpoint {
    Sftp {
        host: String,
        port: u16,
        user: String,
        auth: SshAuth,
    },
    S3 {
        bucket: String,
        /// e.g. "us-east-1". Most S3-compatible services accept any value.
        region: String,
        /// Custom endpoint URL for S3-compatible services (MinIO, R2, Wasabi…).
        /// Empty means Amazon S3.
        #[serde(default)]
        endpoint: Option<String>,
        access_key_id: String,
        /// Address buckets as `endpoint/bucket` instead of `bucket.endpoint`.
        /// Most self-hosted services need this.
        #[serde(default)]
        path_style: bool,
    },
}

/// A favorite path inside a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFavorite {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub endpoint: Endpoint,
    /// Folder to open after connecting. Empty means the home folder (SFTP)
    /// or the bucket root (S3).
    #[serde(default)]
    pub initial_path: Option<String>,
    #[serde(default)]
    pub favorites: Vec<RemoteFavorite>,
}

impl Connection {
    /// Id under which the connected file system is registered.
    pub fn fs_id(&self) -> String {
        fs_id(&self.id)
    }

    /// Whether connecting needs a secret from the store.
    pub fn uses_secret(&self) -> bool {
        match &self.endpoint {
            Endpoint::Sftp { auth, .. } => !matches!(auth, SshAuth::Agent),
            Endpoint::S3 { .. } => true,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("a connection needs a name".into()));
        }
        match &self.endpoint {
            Endpoint::Sftp {
                host, user, port, ..
            } => {
                if host.trim().is_empty() || user.trim().is_empty() || *port == 0 {
                    return Err(Error::Invalid(
                        "SFTP needs a server, a port and a user name".into(),
                    ));
                }
            }
            Endpoint::S3 {
                bucket,
                access_key_id,
                ..
            } => {
                if bucket.trim().is_empty() || access_key_id.trim().is_empty() {
                    return Err(Error::Invalid("S3 needs a bucket and an access key".into()));
                }
            }
        }
        Ok(())
    }
}

/// File system id of a connection, e.g. `conn:3f2a…`.
pub fn fs_id(connection_id: &str) -> String {
    format!("conn:{connection_id}")
}

/// Where connection secrets are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, secret: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// A secret store that only lives in memory, for tests and as a fallback.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The saved connections, persisted as JSON.
pub struct Connections {
    path: Option<PathBuf>,
    items: Mutex<Vec<Connection>>,
    secrets: Arc<dyn SecretStore>,
}

impl Connections {
    pub fn open(path: impl AsRef<Path>, secrets: Arc<dyn SecretStore>) -> Self {
        let path = path.as_ref().to_path_buf();
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            items: Mutex::new(items),
            secrets,
        }
    }

    pub fn in_memory(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            path: None,
            items: Mutex::new(Vec::new()),
            secrets,
        }
    }

    pub fn list(&self) -> Vec<Connection> {
        self.items.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Result<Connection> {
        self.items
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("connection {id}")))
    }

    /// Add or update a connection. An empty `id` creates a new one. `secret`
    /// replaces the stored secret when given; `None` keeps the old one.
    pub fn save(&self, mut conn: Connection, secret: Option<String>) -> Result<Connection> {
        conn.validate()?;
        if conn.id.is_empty() {
            conn.id = uuid::Uuid::new_v4().to_string();
        }
        if let Some(s) = secret {
            if s.is_empty() {
                self.secrets.delete(&conn.id)?;
            } else {
                self.secrets.set(&conn.id, &s)?;
            }
        }
        let mut items = self.items.lock().unwrap();
        match items.iter_mut().find(|c| c.id == conn.id) {
            Some(existing) => {
                // Favorites are edited separately; keep them unless given.
                if conn.favorites.is_empty() {
                    conn.favorites = existing.favorites.clone();
                }
                *existing = conn.clone();
            }
            None => items.push(conn.clone()),
        }
        self.persist(&items)?;
        Ok(conn)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        items.retain(|c| c.id != id);
        self.persist(&items)?;
        drop(items);
        self.secrets.delete(id)
    }

    pub fn reorder(&self, ids: &[String]) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        items.sort_by_key(|c| ids.iter().position(|i| *i == c.id).unwrap_or(usize::MAX));
        self.persist(&items)
    }

    pub fn add_favorite(&self, id: &str, fav: RemoteFavorite) -> Result<Connection> {
        self.update(id, |c| {
            let path = posix::normalize(&fav.path);
            if !c.favorites.iter().any(|f| f.path == path) {
                c.favorites.push(RemoteFavorite { path, ..fav });
            }
        })
    }

    pub fn remove_favorite(&self, id: &str, path: &str) -> Result<Connection> {
        self.update(id, |c| c.favorites.retain(|f| f.path != path))
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Connection)) -> Result<Connection> {
        let mut items = self.items.lock().unwrap();
        let conn = items
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound(format!("connection {id}")))?;
        f(conn);
        let out = conn.clone();
        self.persist(&items)?;
        Ok(out)
    }

    pub fn secret(&self, id: &str) -> Result<Option<String>> {
        self.secrets.get(id)
    }

    fn persist(&self, items: &[Connection]) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
        }
        let json = serde_json::to_vec_pretty(items).map_err(|e| Error::Invalid(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| Error::io(path.to_string_lossy(), e))
    }
}

/// Options for [`connect`] that come from the user at connect time.
#[derive(Clone, Debug, Default)]
pub struct ConnectOptions {
    /// Secret to use instead of the stored one (e.g. typed into a prompt).
    pub secret: Option<String>,
    /// SSH host key fingerprint the user has just confirmed. It is added to
    /// known_hosts when it matches the server's key.
    pub trust_fingerprint: Option<String>,
    /// known_hosts file to use instead of ~/.ssh/known_hosts (tests).
    pub known_hosts: Option<PathBuf>,
}

/// A live connection: the registered file system and the folder to open.
pub struct Connected {
    pub fs: Arc<dyn FileSystem>,
    pub start_path: String,
}

/// Open a connection. The caller registers `fs` in the [`crate::Registry`].
pub async fn connect(
    conn: &Connection,
    secret: Option<String>,
    opts: ConnectOptions,
) -> Result<Connected> {
    let secret = opts.secret.clone().or(secret);
    match &conn.endpoint {
        Endpoint::Sftp {
            host,
            port,
            user,
            auth,
        } => {
            let fs = SftpFs::connect(
                conn.fs_id(),
                conn.name.clone(),
                crate::vfs::SftpParams {
                    host: host.clone(),
                    port: *port,
                    user: user.clone(),
                    auth: auth.clone(),
                    secret,
                    trust_fingerprint: opts.trust_fingerprint,
                    known_hosts: opts.known_hosts,
                },
            )
            .await?;
            let start = match conn
                .initial_path
                .as_deref()
                .filter(|p| !p.trim().is_empty())
            {
                Some(p) => posix::normalize(p),
                None => fs.home().to_string(),
            };
            Ok(Connected {
                fs: Arc::new(fs),
                start_path: start,
            })
        }
        Endpoint::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            path_style,
        } => {
            let secret = secret
                .ok_or_else(|| Error::AuthFailed("the S3 secret access key is missing".into()))?;
            let fs = S3Fs::connect(
                conn.fs_id(),
                conn.name.clone(),
                crate::vfs::S3Params {
                    bucket: bucket.clone(),
                    region: region.clone(),
                    endpoint: endpoint.clone().filter(|e| !e.trim().is_empty()),
                    access_key_id: access_key_id.clone(),
                    secret_access_key: secret,
                    path_style: *path_style,
                },
            )
            .await?;
            let start = posix::normalize(conn.initial_path.as_deref().unwrap_or("/"));
            Ok(Connected {
                fs: Arc::new(fs),
                start_path: start,
            })
        }
    }
}

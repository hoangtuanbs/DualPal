//! SFTP over SSH, built on `russh` (pure Rust, no OpenSSL), so it behaves
//! the same on macOS and Windows.
//!
//! Host keys are checked against `~/.ssh/known_hosts`, like the `ssh`
//! command. An unknown host fails with [`Error::UnknownHost`] carrying the key
//! fingerprint; once the user confirms it, connecting again with that
//! fingerprint in `trust_fingerprint` records the key and goes ahead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{self, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::sync::Mutex;

use super::{posix, Entry, EntryKind, FileSystem, Reader, Writer};
use crate::connections::SshAuth;
use crate::error::{Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Everything needed to (re)open the SSH session.
#[derive(Clone, Debug)]
pub struct SftpParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    pub secret: Option<String>,
    pub trust_fingerprint: Option<String>,
    pub known_hosts: Option<PathBuf>,
}

/// Outcome of the host key check, read after the handshake.
#[derive(Clone, Debug)]
enum HostCheck {
    Unknown(String),
    Changed(String),
}

struct Client {
    host: String,
    port: u16,
    trust: Option<String>,
    known_hosts: Option<PathBuf>,
    outcome: Arc<StdMutex<Option<HostCheck>>>,
}

fn host_key(k: &PublicKeyOrCertificate) -> PublicKey {
    match k {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        PublicKeyOrCertificate::Certificate(c) => PublicKey::new(c.public_key().clone(), ""),
    }
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let key = host_key(server_key);
        let fp = fingerprint(&key);
        let known = match &self.known_hosts {
            Some(p) => keys::check_known_hosts_path(&self.host, self.port, &key, p),
            None => keys::check_known_hosts(&self.host, self.port, &key),
        };
        match known {
            Ok(true) => Ok(true),
            Err(keys::Error::KeyChanged { .. }) => {
                *self.outcome.lock().unwrap() = Some(HostCheck::Changed(fp));
                Ok(false)
            }
            // Not recorded yet (or no known_hosts file at all).
            _ => {
                if self.trust.as_deref() == Some(fp.as_str()) {
                    let learned = match &self.known_hosts {
                        Some(p) => keys::known_hosts::learn_known_hosts_path(
                            &self.host, self.port, &key, p,
                        ),
                        None => keys::known_hosts::learn_known_hosts(&self.host, self.port, &key),
                    };
                    // Failing to write known_hosts must not block a key the user accepted.
                    let _ = learned;
                    Ok(true)
                } else {
                    *self.outcome.lock().unwrap() = Some(HostCheck::Unknown(fp));
                    Ok(false)
                }
            }
        }
    }
}

struct Live {
    handle: Handle<Client>,
    sftp: SftpSession,
}

pub struct SftpFs {
    id: String,
    name: String,
    params: SftpParams,
    home: String,
    live: Mutex<Arc<Live>>,
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

async fn open_live(p: &SftpParams) -> Result<Live> {
    let outcome = Arc::new(StdMutex::new(None));
    let handler = Client {
        host: p.host.clone(),
        port: p.port,
        trust: p.trust_fingerprint.clone(),
        known_hosts: p.known_hosts.clone(),
        outcome: outcome.clone(),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let connecting = client::connect(config, (p.host.as_str(), p.port), handler);
    let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => {
            return Err(Error::Connection(format!(
                "{}:{} did not answer",
                p.host, p.port
            )))
        }
        Ok(Err(e)) => {
            return Err(match outcome.lock().unwrap().take() {
                Some(HostCheck::Unknown(fingerprint)) => Error::UnknownHost {
                    host: p.host.clone(),
                    fingerprint,
                },
                Some(HostCheck::Changed(fingerprint)) => Error::HostKeyChanged {
                    host: p.host.clone(),
                    fingerprint,
                },
                None => Error::Connection(format!("{}:{}: {e}", p.host, p.port)),
            })
        }
        Ok(Ok(h)) => h,
    };

    authenticate(&mut handle, p).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| Error::Connection(format!("could not open a channel: {e}")))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| Error::Connection(format!("the server has no SFTP: {e}")))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| Error::Connection(format!("SFTP did not start: {e}")))?;
    Ok(Live { handle, sftp })
}

async fn authenticate(h: &mut Handle<Client>, p: &SftpParams) -> Result<()> {
    let user = p.user.as_str();
    let ok = match &p.auth {
        SshAuth::Password => {
            let pw = p
                .secret
                .clone()
                .ok_or_else(|| Error::AuthFailed("a password is needed".into()))?;
            password(h, user, &pw).await?
        }
        SshAuth::KeyFile { path } => {
            let key = keys::load_secret_key(expand_home(path), p.secret.as_deref()).map_err(
                |e| match e {
                    keys::Error::KeyIsEncrypted => {
                        Error::AuthFailed("the key file needs its passphrase".into())
                    }
                    e => Error::AuthFailed(format!("could not read the key file {path}: {e}")),
                },
            )?;
            with_key(h, user, key).await?
        }
        SshAuth::Agent => {
            agent(h, user).await? || default_keys(h, user, p.secret.as_deref()).await?
        }
    };
    if ok {
        Ok(())
    } else {
        Err(Error::AuthFailed(format!(
            "the server did not accept the sign-in for {user}"
        )))
    }
}

fn conn_err(e: russh::Error) -> Error {
    Error::Connection(e.to_string())
}

async fn password(h: &mut Handle<Client>, user: &str, pw: &str) -> Result<bool> {
    if h.authenticate_password(user, pw)
        .await
        .map_err(conn_err)?
        .success()
    {
        return Ok(true);
    }
    // Many servers only offer keyboard-interactive; answer its prompts with the password.
    let mut reply = h
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(conn_err)?;
    for _ in 0..4 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| pw.to_string()).collect();
                reply = h
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(conn_err)?;
            }
        }
    }
    Ok(false)
}

async fn with_key(h: &mut Handle<Client>, user: &str, key: keys::PrivateKey) -> Result<bool> {
    let hash = h
        .best_supported_rsa_hash()
        .await
        .map_err(conn_err)?
        .flatten();
    let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
    Ok(h.authenticate_publickey(user, key)
        .await
        .map_err(conn_err)?
        .success())
}

async fn default_keys(
    h: &mut Handle<Client>,
    user: &str,
    passphrase: Option<&str>,
) -> Result<bool> {
    let Some(ssh) = dirs::home_dir().map(|d| d.join(".ssh")) else {
        return Ok(false);
    };
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = ssh.join(name);
        if !path.exists() {
            continue;
        }
        if let Ok(key) = keys::load_secret_key(&path, passphrase) {
            if with_key(h, user, key).await? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn agent(h: &mut Handle<Client>, user: &str) -> Result<bool> {
    #[cfg(unix)]
    let client = keys::agent::client::AgentClient::connect_env()
        .await
        .map(|c| c.dynamic());
    #[cfg(windows)]
    let client =
        match keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
        {
            Ok(c) => Ok(c.dynamic()),
            Err(_) => keys::agent::client::AgentClient::connect_pageant()
                .await
                .map(|c| c.dynamic()),
        };
    let Ok(mut client) = client else {
        return Ok(false);
    };
    let Ok(ids) = client.request_identities().await else {
        return Ok(false);
    };
    let hash = h
        .best_supported_rsa_hash()
        .await
        .map_err(conn_err)?
        .flatten();
    for id in ids {
        let keys::agent::AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        if let Ok(r) = h
            .authenticate_publickey_with(user, key, hash, &mut client)
            .await
        {
            if r.success() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn map_err(path: &str, e: SftpError) -> Error {
    match &e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => Error::NotFound(path.to_string()),
            StatusCode::PermissionDenied => Error::PermissionDenied(path.to_string()),
            _ => Error::Io {
                path: path.to_string(),
                source: std::io::Error::other(s.error_message.clone()),
            },
        },
        _ => Error::Io {
            path: path.to_string(),
            source: std::io::Error::other(e.to_string()),
        },
    }
}

fn ms(secs: Option<u32>) -> Option<i64> {
    secs.map(|s| s as i64 * 1000)
}

fn kind_of(a: &FileAttributes) -> EntryKind {
    if a.is_dir() {
        EntryKind::Dir
    } else if a.is_regular() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn to_entry(
    path: String,
    name: String,
    a: &FileAttributes,
    target: Option<&FileAttributes>,
) -> Entry {
    let symlink = a.is_symlink();
    let eff = target.unwrap_or(a);
    let mode = eff.permissions.map(|m| m & 0o7777);
    Entry {
        hidden: name.starts_with('.'),
        name,
        path,
        kind: if symlink && target.is_none() {
            EntryKind::Other
        } else {
            kind_of(eff)
        },
        symlink,
        size: eff.size.unwrap_or(0),
        modified_ms: ms(eff.mtime),
        created_ms: None,
        readonly: mode.is_some_and(|m| m & 0o222 == 0),
        mode,
    }
}

impl SftpFs {
    pub async fn connect(id: String, name: String, params: SftpParams) -> Result<Self> {
        let live = open_live(&params).await?;
        let home = live
            .sftp
            .canonicalize(".")
            .await
            .map(|h| posix::normalize(&h))
            .unwrap_or_else(|_| "/".to_string());
        Ok(Self {
            id,
            name,
            // The fingerprint is in known_hosts now; reconnects check it there.
            params: SftpParams {
                trust_fingerprint: None,
                ..params
            },
            home,
            live: Mutex::new(Arc::new(live)),
        })
    }

    /// The user's home folder on the server.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// The SFTP session, reconnecting first if the connection dropped.
    async fn sftp(&self) -> Result<Arc<Live>> {
        let mut live = self.live.lock().await;
        if live.handle.is_closed() {
            *live = Arc::new(open_live(&self.params).await?);
        }
        Ok(live.clone())
    }

    pub async fn disconnect(&self) {
        let live = self.live.lock().await;
        let _ = live.sftp.close().await;
        let _ = live
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

#[async_trait]
impl FileSystem for SftpFs {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        self.name.clone()
    }

    fn join(&self, dir: &str, name: &str) -> String {
        posix::join(dir, name)
    }

    fn parent(&self, path: &str) -> Option<String> {
        posix::parent(path)
    }

    fn file_name(&self, path: &str) -> Option<String> {
        posix::file_name(path)
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        posix::is_within(path, ancestor)
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let live = self.sftp().await?;
        let rd = live.sftp.read_dir(dir).await.map_err(|e| map_err(dir, e))?;
        let mut out = Vec::new();
        for item in rd {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let path = posix::join(dir, &name);
            let attrs = item.metadata();
            let target = if attrs.is_symlink() {
                live.sftp.metadata(path.clone()).await.ok()
            } else {
                None
            };
            out.push(to_entry(path, name, &attrs, target.as_ref()));
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        let live = self.sftp().await?;
        let attrs = live
            .sftp
            .symlink_metadata(path)
            .await
            .map_err(|e| map_err(path, e))?;
        let target = if attrs.is_symlink() {
            live.sftp.metadata(path).await.ok()
        } else {
            None
        };
        let name = posix::file_name(path).unwrap_or_else(|| "/".to_string());
        Ok(to_entry(path.to_string(), name, &attrs, target.as_ref()))
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        let live = self.sftp().await?;
        if let Err(e) = live.sftp.create_dir(path).await {
            // SFTP v3 reports "already exists" as a generic failure.
            if live.sftp.symlink_metadata(path).await.is_ok() {
                return Err(Error::AlreadyExists(path.to_string()));
            }
            return Err(map_err(path, e));
        }
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let live = self.sftp().await?;
        live.sftp
            .remove_file(path)
            .await
            .map_err(|e| map_err(path, e))
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        let live = self.sftp().await?;
        live.sftp
            .remove_dir(path)
            .await
            .map_err(|e| map_err(path, e))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // Like a local rename, replace an existing file but never a folder.
        // SFTP v3 refuses to rename onto an existing name, so remove it first.
        let live = self.sftp().await?;
        if let Ok(existing) = live.sftp.symlink_metadata(to).await {
            if existing.is_dir() {
                return Err(Error::AlreadyExists(to.to_string()));
            }
            live.sftp
                .remove_file(to)
                .await
                .map_err(|e| map_err(to, e))?;
        }
        live.sftp
            .rename(from, to)
            .await
            .map_err(|e| map_err(from, e))
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        let live = self.sftp().await?;
        let f = live.sftp.open(path).await.map_err(|e| map_err(path, e))?;
        Ok(Box::new(f))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        let live = self.sftp().await?;
        let f = live
            .sftp
            .open_with_flags(
                path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| map_err(path, e))?;
        Ok(Box::new(f))
    }

    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        let live = self.sftp().await?;
        let attrs = FileAttributes {
            permissions: Some(mode & 0o7777),
            ..FileAttributes::empty()
        };
        live.sftp
            .set_metadata(path, attrs)
            .await
            .map_err(|e| map_err(path, e))
    }

    async fn set_modified(&self, path: &str, modified_ms: i64) -> Result<()> {
        let live = self.sftp().await?;
        let secs = (modified_ms / 1000).clamp(0, u32::MAX as i64) as u32;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(secs);
        let attrs = FileAttributes {
            mtime: Some(secs),
            atime: Some(now),
            ..FileAttributes::empty()
        };
        live.sftp
            .set_metadata(path, attrs)
            .await
            .map_err(|e| map_err(path, e))
    }
}

/// Hosts from an OpenSSH client config file (`~/.ssh/config`), for the
/// "add connection" dialog. Wildcard patterns are skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHost {
    pub alias: String,
    pub host_name: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub fn ssh_config_hosts(path: Option<&Path>) -> Vec<SshConfigHost> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => match dirs::home_dir() {
            Some(h) => h.join(".ssh").join("config"),
            None => return Vec::new(),
        },
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_ssh_config(&text)
}

pub fn parse_ssh_config(text: &str) -> Vec<SshConfigHost> {
    let mut out: Vec<SshConfigHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.to_ascii_lowercase(),
                v.trim_start_matches(['=', ' ', '\t'])
                    .trim()
                    .trim_matches('"'),
            ),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in value.split_whitespace() {
                    if alias.contains(['*', '?', '!']) {
                        continue;
                    }
                    current.push(out.len());
                    out.push(SshConfigHost {
                        alias: alias.to_string(),
                        host_name: alias.to_string(),
                        ..Default::default()
                    });
                }
            }
            "match" => current.clear(),
            "hostname" => current
                .iter()
                .for_each(|&i| out[i].host_name = value.to_string()),
            "user" => current
                .iter()
                .for_each(|&i| out[i].user = Some(value.to_string())),
            "port" => current
                .iter()
                .for_each(|&i| out[i].port = value.parse().ok()),
            "identityfile" => current.iter().for_each(|&i| {
                if out[i].identity_file.is_none() {
                    out[i].identity_file = Some(value.to_string());
                }
            }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ssh_config() {
        let hosts = parse_ssh_config(
            "Host *\n  ServerAliveInterval 30\n\nHost web prod\n  HostName example.com\n  User deploy\n  Port 2222\n  IdentityFile ~/.ssh/id_web\n\nHost=box\nHostName=10.0.0.2\n",
        );
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "web");
        assert_eq!(hosts[1].alias, "prod");
        assert_eq!(hosts[1].host_name, "example.com");
        assert_eq!(hosts[1].port, Some(2222));
        assert_eq!(hosts[0].identity_file.as_deref(), Some("~/.ssh/id_web"));
        assert_eq!(hosts[2].host_name, "10.0.0.2");
        assert_eq!(hosts[2].user, None);
    }
}

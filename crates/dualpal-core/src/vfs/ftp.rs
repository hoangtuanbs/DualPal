//! FTP and FTPS servers.
//!
//! FTPS comes in two kinds: explicit (the usual one: connect on port 21 and
//! switch to TLS with `AUTH TLS`) and implicit (TLS from the first byte, on
//! port 990). Certificates are checked against the system's trusted
//! authorities. Servers on routers and NAS boxes often use a self-signed
//! certificate instead; then connecting fails with [`Error::UnknownHost`]
//! and the certificate's SHA-256 fingerprint, and once the user confirms it,
//! connecting again with that fingerprint in `trust_fingerprint` remembers
//! the certificate for this server, like an SSH host key.
//!
//! FTP allows one transfer per connection, so file listings and other
//! commands share a few pooled connections, while each file being read or
//! written gets a connection of its own.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use suppaftp::tokio::{
    AsyncDataStream, AsyncRustlsConnector, AsyncRustlsFtpStream, AsyncRustlsStream, TransferStream,
};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Status};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{posix, Entry, EntryKind, FileSystem, Reader, Writer};
use crate::error::{Error, Result};

type Stream = AsyncRustlsFtpStream;

/// How long to wait for a server to answer when connecting.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Idle connections kept for listings and other commands.
const POOL_SIZE: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FtpSecurity {
    /// Plain FTP: the password and files travel unencrypted.
    #[default]
    None,
    /// FTPS with `AUTH TLS` on the normal port (21).
    Explicit,
    /// FTPS with TLS from the start, usually on port 990.
    Implicit,
}

#[derive(Clone, Debug)]
pub struct FtpParams {
    pub host: String,
    pub port: u16,
    /// Empty for anonymous FTP.
    pub user: String,
    pub password: Option<String>,
    pub security: FtpSecurity,
    /// Certificate fingerprint the user has just confirmed.
    pub trust_fingerprint: Option<String>,
    /// Where confirmed certificates are remembered; the default is
    /// `known_certificates.json` in the app's data folder.
    pub known_certs: Option<PathBuf>,
}

pub struct FtpFs {
    id: String,
    name: String,
    p: FtpParams,
    tls: Option<Arc<rustls::ClientConfig>>,
    pool: Mutex<Vec<Stream>>,
    home: String,
    mlsd: bool,
    mfmt: bool,
    /// The server announced `REST STREAM`, so uploads can carry on midway.
    rest: bool,
}

/// SHA-256 fingerprint of a certificate, as `SHA256:` and hex pairs.
fn fingerprint(cert: &[u8]) -> String {
    let hex: Vec<String> = Sha256::digest(cert)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    format!("SHA256:{}", hex.join(":"))
}

fn known_certs_path(p: &FtpParams) -> Option<PathBuf> {
    p.known_certs
        .clone()
        .or_else(|| dirs::data_dir().map(|d| d.join("Dual Pal").join("known_certificates.json")))
}

fn load_known(path: &Option<PathBuf>) -> HashMap<String, String> {
    path.as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn remember(path: &Option<PathBuf>, key: &str, fp: &str) {
    let Some(p) = path else { return };
    let mut known = load_known(path);
    known.insert(key.to_string(), fp.to_string());
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&known) {
        let _ = std::fs::write(p, json);
    }
}

/// Accepts certificates the system trusts, and the one certificate the
/// user confirmed for this server. Records what the server presented.
#[derive(Debug)]
struct Verifier {
    webpki: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
    trusted: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity);
        *self.seen.lock().unwrap() = Some(fp.clone());
        if self.trusted.as_deref() == Some(fp.as_str()) {
            return Ok(ServerCertVerified::assertion());
        }
        match &self.webpki {
            Some(v) => v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now),
            None => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn tls_config(
    trusted: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
) -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let webpki = if roots.is_empty() {
        None
    } else {
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
            .build()
            .ok()
    };
    let verifier = Verifier {
        webpki,
        provider: provider.clone(),
        trusted,
        seen,
    };
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Connection(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// The server's reply code, for an error that is the server saying no.
fn status(e: &FtpError) -> Option<u32> {
    match e {
        FtpError::UnexpectedResponse(r) => Some(r.status.code()),
        _ => None,
    }
}

fn reply_text(e: &FtpError) -> String {
    match e {
        FtpError::UnexpectedResponse(r) => {
            let body = String::from_utf8_lossy(&r.body);
            body.trim().to_string()
        }
        other => other.to_string(),
    }
}

fn map_err(path: &str, e: FtpError) -> Error {
    match status(&e) {
        Some(550) | Some(450) => Error::NotFound(format!("{path} ({})", reply_text(&e))),
        Some(553) | Some(552) | Some(532) => {
            Error::PermissionDenied(format!("{path} ({})", reply_text(&e)))
        }
        Some(530) => Error::AuthFailed(reply_text(&e)),
        Some(_) => Error::Invalid(format!("{path}: {}", reply_text(&e))),
        None => Error::Io {
            path: path.to_string(),
            source: std::io::Error::other(e.to_string()),
        },
    }
}

/// Whether the connection is still usable after this result.
fn reusable<T>(r: &std::result::Result<T, FtpError>) -> bool {
    match r {
        Ok(_) => true,
        Err(e) => status(e).is_some(),
    }
}

fn ms(t: SystemTime) -> Option<i64> {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
        .filter(|&m| m > 0)
}

fn to_entry(dir: &str, f: &suppaftp::list::File) -> Option<Entry> {
    let name = f.name().to_string();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return None;
    }
    Some(Entry {
        hidden: name.starts_with('.'),
        path: posix::join(dir, &name),
        kind: if f.is_directory() {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        symlink: f.is_symlink(),
        size: if f.is_directory() { 0 } else { f.size() as u64 },
        modified_ms: ms(f.modified()),
        created_ms: None,
        readonly: false,
        mode: None,
        name,
    })
}

impl FtpFs {
    pub async fn connect(id: String, name: String, p: FtpParams) -> Result<Self> {
        let known_path = known_certs_path(&p);
        let key = format!("{}:{}", p.host, p.port);
        let stored = load_known(&known_path).get(&key).cloned();
        let trusted = p.trust_fingerprint.clone().or(stored.clone());
        let seen = Arc::new(Mutex::new(None));
        let tls = match p.security {
            FtpSecurity::None => None,
            _ => Some(Arc::new(tls_config(trusted.clone(), seen.clone())?)),
        };
        let mut fs = Self {
            id,
            name,
            p,
            tls,
            pool: Mutex::new(Vec::new()),
            home: "/".into(),
            mlsd: false,
            mfmt: false,
            rest: false,
        };
        let mut s = match fs.open().await {
            Ok(s) => s,
            Err(e) => {
                // A certificate nobody vouches for: ask the user.
                let fp = seen.lock().unwrap().clone();
                if let (Some(fp), Error::Connection(_)) = (fp, &e) {
                    if trusted.as_deref() != Some(fp.as_str()) {
                        return Err(match stored {
                            Some(_) => Error::HostKeyChanged {
                                host: fs.p.host.clone(),
                                fingerprint: fp,
                            },
                            None => Error::UnknownHost {
                                host: fs.p.host.clone(),
                                fingerprint: fp,
                            },
                        });
                    }
                }
                return Err(e);
            }
        };
        if let (Some(fp), Some(confirmed)) = (seen.lock().unwrap().clone(), &fs.p.trust_fingerprint)
        {
            if &fp == confirmed {
                remember(&known_path, &key, &fp);
            }
        }
        if let Ok(feat) = s.feat().await {
            let has = |k: &str| feat.keys().any(|f| f.eq_ignore_ascii_case(k));
            fs.mlsd = has("MLST");
            fs.mfmt = has("MFMT");
            fs.rest = feat.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("REST")
                    && v.as_deref()
                        .is_some_and(|v| v.to_ascii_uppercase().contains("STREAM"))
            });
        }
        if let Ok(pwd) = s.pwd().await {
            if pwd.starts_with('/') {
                fs.home = posix::normalize(&pwd);
            }
        }
        fs.pool.lock().unwrap().push(s);
        Ok(fs)
    }

    /// The folder the server put us in after signing in.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// A new signed-in connection.
    async fn open(&self) -> Result<Stream> {
        let addr = format!("{}:{}", self.p.host, self.p.port);
        let connector = || {
            AsyncRustlsConnector::from(tokio_rustls::TlsConnector::from(
                self.tls.clone().expect("TLS settings for FTPS"),
            ))
        };
        let conn_err = |e: FtpError| Error::Connection(format!("{addr}: {e}"));
        let fut = async {
            match self.p.security {
                FtpSecurity::None => Stream::connect(&addr).await.map_err(conn_err),
                FtpSecurity::Explicit => {
                    let s = Stream::connect(&addr).await.map_err(conn_err)?;
                    s.into_secure(connector(), &self.p.host)
                        .await
                        .map_err(conn_err)
                }
                FtpSecurity::Implicit => {
                    let mut s = Stream::connect_secure_implicit(&addr, connector(), &self.p.host)
                        .await
                        .map_err(conn_err)?;
                    // Ask for encrypted transfers too, as explicit FTPS does;
                    // some servers insist, others encrypt them anyway.
                    let _ = s.custom_command("PBSZ 0", &[Status::CommandOk]).await;
                    let _ = s.custom_command("PROT P", &[Status::CommandOk]).await;
                    Ok(s)
                }
            }
        };
        let mut s = tokio::time::timeout(CONNECT_TIMEOUT, fut)
            .await
            .map_err(|_| Error::Connection(format!("{addr} did not answer")))??;
        // Servers behind NAT often announce a private address for passive
        // transfers; use the address we reached them at instead.
        s.set_passive_nat_workaround(true);
        let (user, pass) = if self.p.user.trim().is_empty() {
            ("anonymous".to_string(), "dualpal@".to_string())
        } else {
            (
                self.p.user.clone(),
                self.p.password.clone().unwrap_or_default(),
            )
        };
        s.login(user.as_str(), pass.as_str())
            .await
            .map_err(|e| match status(&e) {
                Some(530) | Some(331) | Some(332) => {
                    Error::AuthFailed(format!("{} refused the user name or password", self.p.host))
                }
                _ => conn_err(e),
            })?;
        s.transfer_type(FileType::Binary).await.map_err(conn_err)?;
        Ok(s)
    }

    /// A pooled connection that still answers, or a new one.
    async fn take(&self) -> Result<Stream> {
        loop {
            let pooled = self.pool.lock().unwrap().pop();
            match pooled {
                Some(mut s) => {
                    if s.noop().await.is_ok() {
                        return Ok(s);
                    }
                }
                None => return self.open().await,
            }
        }
    }

    fn give<T>(&self, s: Stream, r: &std::result::Result<T, FtpError>) {
        if reusable(r) {
            let mut pool = self.pool.lock().unwrap();
            if pool.len() < POOL_SIZE {
                pool.push(s);
            }
        }
    }

    async fn list_raw(&self, dir: &str) -> Result<Vec<Entry>> {
        let dir = posix::normalize(dir);
        let mut s = self.take().await?;
        let r = if self.mlsd {
            s.mlsd(Some(&dir)).await
        } else {
            // Not every server takes a path (with spaces) after LIST, but
            // all of them can change folder. `-a` includes hidden files.
            match s.cwd(&dir).await {
                Ok(()) => match s.list(Some("-a")).await {
                    Ok(l) => Ok(l),
                    Err(e) if status(&e).is_some() => s.list(None).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        };
        self.give(s, &r);
        let lines = r.map_err(|e| map_err(&dir, e))?;
        let mut out = Vec::new();
        for line in &lines {
            let parsed = if self.mlsd {
                suppaftp::list::ListParser::parse_mlsd(line)
            } else {
                suppaftp::list::File::try_from(line.as_str())
            };
            if let Ok(f) = parsed {
                if let Some(e) = to_entry(&dir, &f) {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    /// Run one command on a pooled connection.
    async fn command<T>(
        &self,
        path: &str,
        f: impl for<'a> FnOnce(
            &'a mut Stream,
        ) -> Pin<
            Box<dyn std::future::Future<Output = std::result::Result<T, FtpError>> + Send + 'a>,
        >,
    ) -> Result<T> {
        let mut s = self.take().await?;
        let r = f(&mut s).await;
        self.give(s, &r);
        r.map_err(|e| map_err(path, e))
    }
}

/// A file being downloaded, on its own connection.
struct FtpReader {
    _conn: Stream,
    inner: TransferStream<AsyncRustlsStream>,
}

impl AsyncRead for FtpReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// End an upload and wait for the server's verdict.
///
/// On a plain connection, first close our side and wait for the server to
/// close its side, which it does only once it has read everything. Closing
/// the socket straight away can lose the last bytes still on their way
/// through a relay or NAT (seen with Windows → WSL port forwarding: about
/// 50 KB missing at the end, and the server none the wiser). Encrypted
/// connections already wait like this inside suppaftp.
async fn finish_upload(
    mut w: TransferStream<AsyncRustlsStream>,
) -> std::result::Result<(), FtpError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut drained = false;
    if let AsyncDataStream::Tcp(sock) = w.get_mut() {
        if sock.shutdown().await.is_ok() {
            let mut buf = [0u8; 1024];
            drained = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) => return true,
                        Ok(_) => {}
                        Err(_) => return false,
                    }
                }
            })
            .await
            .unwrap_or(false);
        }
    }
    match w.finish().await {
        // `finish` closes our side again; once both sides are closed some
        // systems answer that with "not connected". The server has already
        // read the whole file by then: it only closes after our end of file.
        Err(FtpError::ConnectionError(e))
            if drained && e.kind() == std::io::ErrorKind::NotConnected =>
        {
            Ok(())
        }
        r => r,
    }
}

type Finish = Pin<Box<dyn std::future::Future<Output = std::result::Result<(), FtpError>> + Send>>;

/// A file being uploaded, on its own connection. Shutting it down waits
/// for the server to confirm that the whole file arrived.
struct FtpWriter {
    _conn: Stream,
    inner: Option<TransferStream<AsyncRustlsStream>>,
    finishing: Option<Finish>,
}

impl AsyncWrite for FtpWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.inner.as_mut() {
            Some(w) => Pin::new(w).poll_write(cx, buf),
            None => Poll::Ready(Err(std::io::Error::other("the upload has finished"))),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.inner.as_mut() {
            Some(w) => Pin::new(w).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.finishing.is_none() {
            let Some(w) = self.inner.take() else {
                return Poll::Ready(Ok(()));
            };
            self.finishing = Some(Box::pin(finish_upload(w)));
        }
        let fut = self.finishing.as_mut().unwrap();
        match fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(r) => {
                self.finishing = None;
                Poll::Ready(r.map_err(|e| std::io::Error::other(reply_text(&e))))
            }
        }
    }
}

#[async_trait]
impl FileSystem for FtpFs {
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
        self.list_raw(dir).await
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        let p = posix::normalize(path);
        let Some(parent) = posix::parent(&p) else {
            return Ok(Entry {
                name: "/".into(),
                path: p,
                kind: EntryKind::Dir,
                symlink: false,
                size: 0,
                modified_ms: None,
                created_ms: None,
                hidden: false,
                readonly: false,
                mode: None,
            });
        };
        let name = posix::file_name(&p).unwrap_or_default();
        self.list_raw(&parent)
            .await?
            .into_iter()
            .find(|e| e.name == name)
            .ok_or(Error::NotFound(path.to_string()))
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        if self.exists(path).await? {
            return Err(Error::AlreadyExists(path.to_string()));
        }
        let p = posix::normalize(path);
        self.command(&p, |s| Box::pin(s.mkdir(p.clone()))).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let p = posix::normalize(path);
        self.command(&p, |s| Box::pin(s.rm(p.clone()))).await
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        let p = posix::normalize(path);
        if p == "/" {
            return Err(Error::Invalid(
                "cannot remove the server's root folder".into(),
            ));
        }
        self.command(&p, |s| Box::pin(s.rmdir(p.clone()))).await
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // Like a local rename, replace an existing file but never a folder.
        let entry = self.stat(from).await?;
        match self.stat(to).await {
            Ok(t) if t.is_dir() || entry.is_dir() => {
                return Err(Error::AlreadyExists(to.to_string()))
            }
            Ok(_) => {
                // Not every server replaces the target on rename, so move
                // it aside first, and back if the rename fails.
                let t = posix::normalize(to);
                let dir = posix::parent(&t).unwrap_or_else(|| "/".into());
                let aside = posix::join(
                    &dir,
                    &format!(
                        ".dualpal-old-{}",
                        &uuid::Uuid::new_v4().simple().to_string()[..12]
                    ),
                );
                let f = posix::normalize(from);
                {
                    let (t, aside) = (t.clone(), aside.clone());
                    self.command(&t, |s| Box::pin(s.rename(t.clone(), aside.clone())))
                        .await?;
                }
                let moved = {
                    let (f, t) = (f.clone(), t.clone());
                    self.command(&f, |s| Box::pin(s.rename(f.clone(), t.clone())))
                        .await
                };
                if let Err(e) = moved {
                    let (t, aside) = (t.clone(), aside.clone());
                    let _ = self
                        .command(&aside, |s| Box::pin(s.rename(aside.clone(), t.clone())))
                        .await;
                    return Err(e);
                }
                let _ = self.remove_file(&aside).await;
                return Ok(());
            }
            Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let (f, t) = (posix::normalize(from), posix::normalize(to));
        self.command(&f, |s| Box::pin(s.rename(f.clone(), t.clone())))
            .await
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        self.open_read_at(path, 0).await
    }

    async fn open_read_at(&self, path: &str, offset: u64) -> Result<Reader> {
        let p = posix::normalize(path);
        let mut conn = self.open().await?;
        if offset > 0 {
            conn.resume_transfer(offset as usize)
                .await
                .map_err(|e| map_err(&p, e))?;
        }
        let inner = conn.retr_as_stream(&p).await.map_err(|e| map_err(&p, e))?;
        Ok(Box::new(FtpReader { _conn: conn, inner }))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        let p = posix::normalize(path);
        let mut conn = self.open().await?;
        let inner = conn.put_with_stream(&p).await.map_err(|e| map_err(&p, e))?;
        Ok(Box::new(FtpWriter {
            _conn: conn,
            inner: Some(inner),
            finishing: None,
        }))
    }

    async fn open_write_at(&self, path: &str, offset: u64) -> Result<Writer> {
        // REST then STOR: the server keeps the first `offset` bytes.
        let p = posix::normalize(path);
        let mut conn = self.open().await?;
        conn.resume_transfer(offset as usize)
            .await
            .map_err(|e| map_err(&p, e))?;
        let inner = conn.put_with_stream(&p).await.map_err(|e| map_err(&p, e))?;
        Ok(Box::new(FtpWriter {
            _conn: conn,
            inner: Some(inner),
            finishing: None,
        }))
    }

    fn resumable_writes(&self) -> bool {
        self.rest
    }

    fn parallel_transfers(&self) -> usize {
        // Many servers allow only a few connections from one address.
        2
    }

    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        let p = posix::normalize(path);
        let cmd = format!("CHMOD {:o} {p}", mode & 0o7777);
        self.command(&p, |s| {
            Box::pin(async move { s.site(cmd).await.map(|_| ()) })
        })
        .await
    }

    async fn set_modified(&self, path: &str, modified_ms: i64) -> Result<()> {
        if !self.mfmt {
            return Ok(());
        }
        let Some(t) = chrono::DateTime::from_timestamp_millis(modified_ms) else {
            return Ok(());
        };
        let p = posix::normalize(path);
        let cmd = format!("MFMT {} {p}", t.format("%Y%m%d%H%M%S"));
        // Best effort: an older time is only a nicety.
        let _ = self
            .command(&p, |s| {
                Box::pin(async move { s.custom_command(cmd, &[Status::File]).await.map(|_| ()) })
            })
            .await;
        Ok(())
    }
}

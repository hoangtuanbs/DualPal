//! Streaming uploads to a WebDAV server.
//!
//! OpenDAL sends a WebDAV file in one request built from the whole file in
//! memory. Here the file streams to the server as it is written, so a large
//! upload needs no more memory than a small one.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::PollSender;

use crate::error::{Error, Result};

/// Where uploads go, and how to sign in.
#[derive(Clone)]
pub struct DavTarget {
    pub client: reqwest::Client,
    /// Server address and folder, without a trailing slash, e.g.
    /// `https://host/remote.php/dav/files/me`.
    pub base: String,
    pub user: String,
    pub password: String,
}

/// `a b/c#d` → `a%20b/c%23d`: everything but unreserved characters and `/`.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl DavTarget {
    pub fn url(&self, path: &str) -> String {
        let p = format!("/{}", path.trim_start_matches('/'));
        format!("{}{}", self.base.trim_end_matches('/'), encode_path(&p))
    }

    /// Start uploading `path`; the returned writer streams to the server.
    pub fn put(&self, path: &str) -> DavWriter {
        let (tx, mut rx) = mpsc::channel::<std::io::Result<Bytes>>(4);
        let body = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
        let mut req = self
            .client
            .put(self.url(path))
            .body(reqwest::Body::wrap_stream(body));
        if !self.user.is_empty() {
            req = req.basic_auth(&self.user, Some(&self.password));
        }
        let path = path.to_string();
        let task = tokio::spawn(async move {
            let res = req.send().await.map_err(|e| Error::Io {
                path: path.clone(),
                source: std::io::Error::other(e.to_string()),
            })?;
            let status = res.status();
            if status.is_success() {
                Ok(())
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                Err(Error::PermissionDenied(format!("{path} ({status})")))
            } else if status.as_u16() == 507 {
                Err(Error::Io {
                    path,
                    source: std::io::Error::other("the server is out of space"),
                })
            } else {
                Err(Error::Io {
                    path,
                    source: std::io::Error::other(format!("the server answered {status}")),
                })
            }
        });
        DavWriter {
            tx: Some(PollSender::new(tx)),
            task: Some(task),
        }
    }
}

pub struct DavWriter {
    tx: Option<PollSender<std::io::Result<Bytes>>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl DavWriter {
    /// The upload ended early: report why.
    fn failed(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Error> {
        match self.task.as_mut() {
            Some(t) => match Pin::new(t).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(r) => {
                    self.task = None;
                    Poll::Ready(std::io::Error::other(match r {
                        Ok(Err(e)) => e.to_string(),
                        Ok(Ok(())) => "the server ended the upload early".into(),
                        Err(e) => e.to_string(),
                    }))
                }
            },
            None => Poll::Ready(std::io::Error::other("the upload has finished")),
        }
    }
}

use std::future::Future;

impl AsyncWrite for DavWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = &mut *self;
        let Some(tx) = this.tx.as_mut() else {
            return Poll::Ready(Err(std::io::Error::other("the upload has finished")));
        };
        match tx.poll_reserve(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => {
                let _ = tx.send_item(Ok(Bytes::copy_from_slice(buf)));
                Poll::Ready(Ok(buf.len()))
            }
            // The request is gone: the server refused or the connection broke.
            Poll::Ready(Err(_)) => this.failed(cx).map(Err),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Closing the channel ends the request body; then wait for the answer.
        self.tx = None;
        match self.task.as_mut() {
            None => Poll::Ready(Ok(())),
            Some(t) => match Pin::new(t).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(r) => {
                    self.task = None;
                    Poll::Ready(match r {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(e)) => Err(std::io::Error::other(e.to_string())),
                        Err(e) => Err(std::io::Error::other(e.to_string())),
                    })
                }
            },
        }
    }
}

impl Drop for DavWriter {
    fn drop(&mut self) {
        // Dropped before shutdown (a cancelled copy): abandon the request so
        // the server never stores a cut-off file as complete.
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// A client for one server.
pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| Error::Connection(e.to_string()))
}

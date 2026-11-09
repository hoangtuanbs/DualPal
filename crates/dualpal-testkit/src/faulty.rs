//! A [`FileSystem`] wrapper that injects the failures a real network or disk
//! produces: a write that dies partway through, a broken disk, latency, a
//! rename that refuses to replace, or a one-off error on any call.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use dualpal_core::vfs::{Entry, FileSystem, Reader, Writer};
use dualpal_core::{Error, Result};
use tokio::io::{AsyncWrite, ReadBuf};

/// Which call a fault applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaultOp {
    OpenRead,
    OpenReadAt,
    OpenWrite,
    OpenWriteAt,
    List,
    Stat,
    CreateDir,
    RemoveFile,
    RemoveDir,
    Rename,
    SetMode,
    SetModified,
    Trash,
}

const NEVER: u64 = u64::MAX;

/// Wraps another file system and lets a test inject failures.
pub struct FaultyFs<F> {
    inner: F,
    id: Option<String>,
    state: Arc<State>,
}

struct State {
    /// Bytes the next write may accept before failing; `NEVER` means no fault.
    fail_after: AtomicU64,
    /// Every write fails to open, as on a full disk.
    broken: AtomicBool,
    latency: Mutex<HashMap<FaultOp, Duration>>,
    fail_next: Mutex<Vec<(FaultOp, Error)>>,
    open_writers: AtomicUsize,
    most_writers: AtomicUsize,
    resumed: Mutex<Vec<u64>>,
    reads: AtomicUsize,
    /// 0 = use the wrapped backend, 1 = force true, 2 = force false.
    resumable: AtomicU64,
    atomic: AtomicU64,
    /// 0 = use the wrapped backend.
    parallel: AtomicUsize,
    rename_replaces: AtomicBool,
}

impl<F: FileSystem> FaultyFs<F> {
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            id: None,
            state: Arc::new(State {
                fail_after: AtomicU64::new(NEVER),
                broken: AtomicBool::new(false),
                latency: Mutex::new(HashMap::new()),
                fail_next: Mutex::new(Vec::new()),
                open_writers: AtomicUsize::new(0),
                most_writers: AtomicUsize::new(0),
                resumed: Mutex::new(Vec::new()),
                reads: AtomicUsize::new(0),
                resumable: AtomicU64::new(0),
                atomic: AtomicU64::new(0),
                parallel: AtomicUsize::new(0),
                rename_replaces: AtomicBool::new(true),
            }),
        }
    }

    /// Wrap `inner` but register under `id` (e.g. `"remote"` while the
    /// wrapped disk is the local one).
    pub fn with_id(inner: F, id: impl Into<String>) -> Self {
        let mut this = Self::new(inner);
        this.id = Some(id.into());
        this
    }

    /// The next write stops after `bytes` bytes. `0` fails on the first
    /// write. The fault is used up by one write.
    pub fn fail_write_after(&self, bytes: u64) {
        self.state.fail_after.store(bytes, Ordering::SeqCst);
    }

    /// Every write from now on fails to open (a full disk, not a dropped
    /// connection), so the queue must give up instead of retrying.
    pub fn break_writes(&self) {
        self.state.broken.store(true, Ordering::SeqCst);
    }

    pub fn heal_writes(&self) {
        self.state.broken.store(false, Ordering::SeqCst);
    }

    /// Fail the next call of `op` with `err`.
    pub fn fail_next(&self, op: FaultOp, err: Error) {
        self.state.fail_next.lock().unwrap().push((op, err));
    }

    /// Sleep before every call of `op`.
    pub fn latency(&self, op: FaultOp, delay: Duration) {
        self.state.latency.lock().unwrap().insert(op, delay);
    }

    /// Writers open right now (a file being copied is one writer).
    pub fn open_writers(&self) -> usize {
        self.state.open_writers.load(Ordering::SeqCst)
    }

    /// The most writers that were open at the same time.
    pub fn most_parallel_writers(&self) -> usize {
        self.state.most_writers.load(Ordering::SeqCst)
    }

    /// Offsets passed to `open_write_at`, in order.
    pub fn resumed_offsets(&self) -> Vec<u64> {
        self.state.resumed.lock().unwrap().clone()
    }

    /// How many files were opened for reading (CMP-003).
    pub fn read_count(&self) -> usize {
        self.state.reads.load(Ordering::SeqCst)
    }

    pub fn reset_counts(&self) {
        self.state.most_writers.store(0, Ordering::SeqCst);
        self.state.reads.store(0, Ordering::SeqCst);
        self.state.resumed.lock().unwrap().clear();
    }

    pub fn set_resumable(&self, yes: bool) {
        self.state
            .resumable
            .store(if yes { 1 } else { 2 }, Ordering::SeqCst);
    }

    pub fn set_atomic_writes(&self, yes: bool) {
        self.state
            .atomic
            .store(if yes { 1 } else { 2 }, Ordering::SeqCst);
    }

    pub fn set_parallel_transfers(&self, n: usize) {
        self.state.parallel.store(n.max(1), Ordering::SeqCst);
    }

    /// Whether a rename onto an existing name replaces it (POSIX) or fails
    /// (some servers). Defaults to replacing.
    pub fn set_rename_replaces(&self, yes: bool) {
        self.state.rename_replaces.store(yes, Ordering::SeqCst);
    }

    pub fn inner(&self) -> &F {
        &self.inner
    }

    async fn before(&self, op: FaultOp) -> Result<()> {
        // Read the delay in its own statement so the mutex guard is dropped
        // before the await; holding it would make every future non-`Send`.
        let delay = self.state.latency.lock().unwrap().get(&op).copied();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut queue = self.state.fail_next.lock().unwrap();
        if let Some(i) = queue.iter().position(|(o, _)| *o == op) {
            let (_, err) = queue.remove(i);
            return Err(err);
        }
        Ok(())
    }

    fn budget(&self) -> u64 {
        self.state.fail_after.swap(NEVER, Ordering::SeqCst)
    }

    fn wrap_writer(&self, inner: Writer, budget: u64, path: &str) -> Writer {
        let n = self.state.open_writers.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.most_writers.fetch_max(n, Ordering::SeqCst);
        Box::new(CountingWriter {
            inner,
            budget,
            state: self.state.clone(),
            path: path.to_string(),
        })
    }

    fn broken_error(path: &str) -> Error {
        Error::Io {
            path: path.to_string(),
            source: std::io::Error::other("no space left on device"),
        }
    }
}

struct CountingWriter {
    inner: Writer,
    budget: u64,
    state: Arc<State>,
    path: String,
}

impl Drop for CountingWriter {
    fn drop(&mut self) {
        self.state.open_writers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AsyncWrite for CountingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.budget != NEVER {
            if self.budget == 0 {
                return Poll::Ready(Err(std::io::Error::other(format!(
                    "connection lost writing {}",
                    self.path
                ))));
            }
            let len = buf.len().min(self.budget as usize);
            let r = Pin::new(&mut self.inner).poll_write(cx, &buf[..len]);
            if let Poll::Ready(Ok(n)) = r {
                self.budget -= n as u64;
            }
            return r;
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A reader that keeps the wrapped reader type explicit (a place for future
/// read faults and bandwidth limits).
struct CountingReader {
    inner: Reader,
}

impl tokio::io::AsyncRead for CountingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[async_trait]
impl<F: FileSystem> FileSystem for FaultyFs<F> {
    fn id(&self) -> &str {
        self.id.as_deref().unwrap_or_else(|| self.inner.id())
    }

    fn display_name(&self) -> String {
        self.inner.display_name()
    }

    fn join(&self, dir: &str, name: &str) -> String {
        self.inner.join(dir, name)
    }

    fn parent(&self, path: &str) -> Option<String> {
        self.inner.parent(path)
    }

    fn file_name(&self, path: &str) -> Option<String> {
        self.inner.file_name(path)
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        self.inner.is_within(path, ancestor)
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        self.before(FaultOp::List).await?;
        self.inner.list(dir).await
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        self.before(FaultOp::Stat).await?;
        self.inner.stat(path).await
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        self.before(FaultOp::CreateDir).await?;
        self.inner.create_dir(path).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.before(FaultOp::RemoveFile).await?;
        self.inner.remove_file(path).await
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        self.before(FaultOp::RemoveDir).await?;
        self.inner.remove_dir(path).await
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.before(FaultOp::Rename).await?;
        if !self.state.rename_replaces.load(Ordering::SeqCst) && self.inner.exists(to).await? {
            return Err(Error::AlreadyExists(to.to_string()));
        }
        self.inner.rename(from, to).await
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        self.before(FaultOp::OpenRead).await?;
        self.state.reads.fetch_add(1, Ordering::SeqCst);
        let reader = self.inner.open_read(path).await?;
        Ok(Box::new(CountingReader { inner: reader }))
    }

    async fn open_read_at(&self, path: &str, offset: u64) -> Result<Reader> {
        self.before(FaultOp::OpenReadAt).await?;
        self.state.reads.fetch_add(1, Ordering::SeqCst);
        let reader = self.inner.open_read_at(path, offset).await?;
        Ok(Box::new(CountingReader { inner: reader }))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        self.before(FaultOp::OpenWrite).await?;
        if self.state.broken.load(Ordering::SeqCst) {
            return Err(Self::broken_error(path));
        }
        let budget = self.budget();
        let writer = self.inner.open_write(path).await?;
        Ok(self.wrap_writer(writer, budget, path))
    }

    async fn open_write_at(&self, path: &str, offset: u64) -> Result<Writer> {
        self.before(FaultOp::OpenWriteAt).await?;
        self.state.resumed.lock().unwrap().push(offset);
        if self.state.broken.load(Ordering::SeqCst) {
            return Err(Self::broken_error(path));
        }
        let budget = self.budget();
        let writer = self.inner.open_write_at(path, offset).await?;
        Ok(self.wrap_writer(writer, budget, path))
    }

    fn resumable_writes(&self) -> bool {
        match self.state.resumable.load(Ordering::SeqCst) {
            1 => true,
            2 => false,
            _ => self.inner.resumable_writes(),
        }
    }

    fn atomic_writes(&self) -> bool {
        match self.state.atomic.load(Ordering::SeqCst) {
            1 => true,
            2 => false,
            _ => self.inner.atomic_writes(),
        }
    }

    fn parallel_transfers(&self) -> usize {
        match self.state.parallel.load(Ordering::SeqCst) {
            0 => self.inner.parallel_transfers(),
            n => n,
        }
    }

    async fn trash(&self, path: &str) -> Result<()> {
        self.before(FaultOp::Trash).await?;
        self.inner.trash(path).await
    }

    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        self.before(FaultOp::SetMode).await?;
        self.inner.set_mode(path, mode).await
    }

    async fn set_modified(&self, path: &str, modified_ms: i64) -> Result<()> {
        self.before(FaultOp::SetModified).await?;
        self.inner.set_modified(path, modified_ms).await
    }
}

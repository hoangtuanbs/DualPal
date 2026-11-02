//! Copies to remote storage: temporary names, several files at once, and
//! carrying on after a dropped connection. A local folder stands in for the
//! server, with a write that fails partway through.

use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use dualpal_core::vfs::{LocalFs, Reader, Writer};
use dualpal_core::{
    ActivityLog, ConflictPolicy, Entry, FileSystem, Location, Operation, OperationQueue,
    OperationStatus, Registry, Result,
};
use tokio::io::AsyncWrite;

/// A "remote" disk: the local one, under another id, whose next write stops
/// with an error after `fail_after` bytes (0 = never).
struct Remote {
    local: LocalFs,
    fail_after: AtomicU64,
    open_writers: Arc<AtomicUsize>,
    most_writers: Arc<AtomicUsize>,
    resumed_at: Mutex<Vec<u64>>,
    /// Fail every write, as a full disk would, without retrying.
    broken: std::sync::atomic::AtomicBool,
}

impl Remote {
    fn new(fail_after: u64) -> Arc<Self> {
        Arc::new(Self {
            local: LocalFs::new(),
            fail_after: AtomicU64::new(fail_after),
            open_writers: Arc::default(),
            most_writers: Arc::default(),
            resumed_at: Mutex::default(),
            broken: Default::default(),
        })
    }

    fn wrap(&self, inner: Writer) -> Writer {
        let n = self.open_writers.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_writers.fetch_max(n, Ordering::SeqCst);
        Box::new(Flaky {
            inner,
            budget: self.fail_after.swap(0, Ordering::SeqCst),
            open: self.open_writers.clone(),
        })
    }
}

struct Flaky {
    inner: Writer,
    budget: u64, // bytes left before the "connection drops"; 0 = no limit
    open: Arc<AtomicUsize>,
}

impl Drop for Flaky {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AsyncWrite for Flaky {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut len = buf.len();
        if self.budget > 0 {
            if self.budget == 1 {
                return Poll::Ready(Err(std::io::Error::other("connection lost")));
            }
            len = len.min(self.budget as usize - 1);
        }
        let r = Pin::new(&mut self.inner).poll_write(cx, &buf[..len]);
        if let Poll::Ready(Ok(n)) = r {
            if self.budget > 0 {
                self.budget -= n as u64;
            }
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[async_trait]
impl FileSystem for Remote {
    fn id(&self) -> &str {
        "remote"
    }
    fn display_name(&self) -> String {
        "Remote".into()
    }
    fn join(&self, dir: &str, name: &str) -> String {
        self.local.join(dir, name)
    }
    fn parent(&self, path: &str) -> Option<String> {
        self.local.parent(path)
    }
    fn file_name(&self, path: &str) -> Option<String> {
        self.local.file_name(path)
    }
    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        self.local.is_within(path, ancestor)
    }
    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        self.local.list(dir).await
    }
    async fn stat(&self, path: &str) -> Result<Entry> {
        self.local.stat(path).await
    }
    async fn create_dir(&self, path: &str) -> Result<()> {
        self.local.create_dir(path).await
    }
    async fn remove_file(&self, path: &str) -> Result<()> {
        self.local.remove_file(path).await
    }
    async fn remove_dir(&self, path: &str) -> Result<()> {
        self.local.remove_dir(path).await
    }
    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.local.rename(from, to).await
    }
    async fn set_mode(&self, path: &str, mode: u32) -> Result<()> {
        self.local.set_mode(path, mode).await
    }
    async fn open_read(&self, path: &str) -> Result<Reader> {
        self.local.open_read(path).await
    }
    async fn open_write(&self, path: &str) -> Result<Writer> {
        let w = self.wrap(self.local.open_write(path).await?);
        if self.broken.load(Ordering::SeqCst) {
            return Err(dualpal_core::Error::PermissionDenied(path.into()));
        }
        Ok(w)
    }
    async fn open_write_at(&self, path: &str, offset: u64) -> Result<Writer> {
        self.resumed_at.lock().unwrap().push(offset);
        Ok(self.wrap(self.local.open_write_at(path, offset).await?))
    }
    fn parallel_transfers(&self) -> usize {
        4
    }
}

fn setup(remote: &Arc<Remote>) -> OperationQueue {
    let registry = Registry::with_local();
    registry.register(remote.clone());
    OperationQueue::new(Arc::new(registry), Arc::new(ActivityLog::in_memory(100)), 2)
}

fn copy(src: &Path, dst: &Path, conflict: ConflictPolicy) -> Operation {
    Operation::Copy {
        sources: vec![Location::new("local", src.to_string_lossy())],
        destination: Location::new("remote", dst.to_string_lossy()),
        conflict,
    }
}

/// Bytes that differ at every position, so a misplaced piece shows.
fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("dualpal"))
        .collect()
}

#[tokio::test]
async fn copies_several_files_at_once_without_leftovers() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    for i in 0..12 {
        std::fs::write(src.join(format!("f{i}.bin")), data(3 * 1024 * 1024)).unwrap();
    }
    // An existing file is replaced in one step.
    std::fs::create_dir_all(dst.join("src")).unwrap();
    std::fs::write(dst.join("src/f0.bin"), "old").unwrap();

    let remote = Remote::new(0);
    let q = setup(&remote);
    let id = q.enqueue(copy(&src, &dst, ConflictPolicy::Overwrite));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(info.progress.files_done, 12);
    assert_eq!(info.progress.bytes_done, info.progress.bytes_total);
    for i in 0..12 {
        let got = std::fs::read(dst.join(format!("src/f{i}.bin"))).unwrap();
        assert!(got == data(3 * 1024 * 1024), "f{i} differs");
    }
    assert!(leftovers(&dst.join("src")).is_empty());
    let most = remote.most_writers.load(Ordering::SeqCst);
    assert!(most > 1 && most <= 4, "{most} files were written at once");
}

#[tokio::test]
async fn carries_on_after_a_dropped_connection() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("big.bin");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    let len = 12 * 1024 * 1024;
    std::fs::write(&src, data(len)).unwrap();

    let remote = Remote::new(10 * 1024 * 1024 + 123);
    let q = setup(&remote);
    let id = q.enqueue(copy(&src, &dst, ConflictPolicy::Fail));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert!(std::fs::read(dst.join("big.bin")).unwrap() == data(len));
    assert_eq!(info.progress.bytes_done, len as u64);
    let resumed = remote.resumed_at.lock().unwrap().clone();
    assert_eq!(resumed.len(), 1, "one retry");
    assert!(resumed[0] > 0, "the retry kept what was already sent");
    assert!(leftovers(&dst).is_empty());
}

#[tokio::test]
async fn a_failed_copy_leaves_the_old_file_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("doc.txt");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::write(dst.join("doc.txt"), "old").unwrap();
    // Someone else's file with the name an older version used: not touched.
    std::fs::write(dst.join(".doc.txt.dualpal-part"), "theirs").unwrap();

    let remote = Remote::new(0);
    remote.broken.store(true, Ordering::SeqCst);
    let q = setup(&remote);
    let id = q.enqueue(copy(&src, &dst, ConflictPolicy::Overwrite));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(std::fs::read_to_string(dst.join("doc.txt")).unwrap(), "old");
    assert_eq!(leftovers(&dst), vec![".doc.txt.dualpal-part".to_string()]);
    assert_eq!(
        std::fs::read_to_string(dst.join(".doc.txt.dualpal-part")).unwrap(),
        "theirs"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn overwriting_keeps_the_old_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("run.sh");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(&src, "new").unwrap();
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(dst.join("run.sh"), "old").unwrap();
    std::fs::set_permissions(dst.join("run.sh"), std::fs::Permissions::from_mode(0o700)).unwrap();

    let remote = Remote::new(0);
    let q = setup(&remote);
    let id = q.enqueue(copy(&src, &dst, ConflictPolicy::Overwrite));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(std::fs::read_to_string(dst.join("run.sh")).unwrap(), "new");
    let mode = std::fs::metadata(dst.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[tokio::test]
async fn names_differing_only_in_case_are_copied_one_at_a_time() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    for name in ["a.txt", "A.txt", "b.txt", "c.txt"] {
        std::fs::write(src.join(name), name).unwrap();
    }

    let remote = Remote::new(0);
    let q = setup(&remote);
    let id = q.enqueue(copy(&src, &dst, ConflictPolicy::Skip));
    let info = q.wait(id).await.unwrap();

    assert_eq!(info.status, OperationStatus::Completed, "{:?}", info.error);
    assert_eq!(remote.most_writers.load(Ordering::SeqCst), 1);
}

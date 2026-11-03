//! Executors for each [`Operation`] kind.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{ConflictPolicy, Inner, Operation, Progress};
use crate::compare::{self, Side};
use crate::error::{Error, Result};
use crate::rename::RenameItem;
use crate::sync::{SnapPair, SyncAction, SyncDone, SyncStep};
use crate::vfs::{Entry, EntryKind, FileSystem, LocalFs, Location};

const CHUNK: usize = 256 * 1024;
/// Minimum time between progress broadcasts, so a fast copy of many small
/// files does not flood the UI.
const EMIT_INTERVAL: Duration = Duration::from_millis(100);

type Fs = Arc<dyn FileSystem>;

/// One step of a copy/move plan.
struct Item {
    src_fs: Fs,
    src: String,
    dst: String,
    dir: bool,
    size: u64,
    modified_ms: Option<i64>,
}

struct Job {
    inner: Arc<Inner>,
    id: Uuid,
    cancel: CancellationToken,
    // Locks rather than plain fields, so that several files can be copied at
    // once. Never held across an await.
    progress: Mutex<Progress>,
    last_emit: Mutex<Instant>,
}

pub(super) async fn run(
    inner: Arc<Inner>,
    id: Uuid,
    cancel: CancellationToken,
    op: Operation,
) -> Result<()> {
    let mut job = Job {
        inner,
        id,
        cancel,
        progress: Mutex::new(Progress::default()),
        last_emit: Mutex::new(Instant::now()),
    };
    let result = match op {
        Operation::Copy {
            sources,
            destination,
            conflict,
        } => job.transfer(sources, destination, conflict, false).await,
        Operation::Move {
            sources,
            destination,
            conflict,
        } => job.transfer(sources, destination, conflict, true).await,
        Operation::Trash { targets } => job.trash(targets).await,
        Operation::Delete { targets } => job.delete(targets).await,
        Operation::Compress {
            sources,
            destination,
        } => job.compress(sources, destination).await,
        Operation::SetMode { location, mode } => job.set_mode(location, mode).await,
        Operation::CreateDir { location } => job.create_dir(location).await,
        Operation::Rename { location, new_name } => job.rename(location, new_name).await,
        Operation::BatchRename { items } => job.batch_rename(items).await,
        Operation::Sync { left, right, steps } => job.sync(left, right, steps).await,
    };
    job.emit(true);
    result
}

impl Job {
    fn fs(&self, id: &str) -> Result<Fs> {
        self.inner.registry.get(id)
    }

    fn check(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    fn prog(&self) -> MutexGuard<'_, Progress> {
        self.progress.lock().unwrap()
    }

    fn emit(&self, force: bool) {
        {
            let mut last = self.last_emit.lock().unwrap();
            if !force && last.elapsed() < EMIT_INTERVAL {
                return;
            }
            *last = Instant::now();
        }
        let p = self.prog().clone();
        self.inner.update(self.id, |i| i.progress = p);
    }

    fn set_current(&self, path: &str) {
        self.prog().current = Some(path.to_string());
        self.emit(false);
    }

    // ---- copy / move -------------------------------------------------------

    async fn transfer(
        &mut self,
        sources: Vec<Location>,
        dest: Location,
        conflict: ConflictPolicy,
        is_move: bool,
    ) -> Result<()> {
        let dst_fs = self.fs(&dest.fs)?;
        if !dst_fs.stat(&dest.path).await?.is_dir() {
            return Err(Error::Invalid(format!(
                "destination is not a folder: {}",
                dest.path
            )));
        }

        let mut plan = Vec::new();
        // Source roots of a cross-device move, removed once their contents are copied.
        let mut move_roots: Vec<(Fs, String)> = Vec::new();

        for src in &sources {
            self.check()?;
            let src_fs = self.fs(&src.fs)?;
            let entry = src_fs.stat(&src.path).await?;
            let name = src_fs
                .file_name(&src.path)
                .ok_or_else(|| Error::Invalid(format!("cannot copy {}", src.path)))?;
            check_name(&dst_fs, &name)?;
            let target = dst_fs.join(&dest.path, &name);
            let same_fs = src.fs == dest.fs;

            if same_fs && target == src.path {
                if is_move {
                    continue; // moving an item into the folder it is already in
                }
                return Err(Error::Invalid(format!(
                    "source and destination are the same: {}",
                    src.path
                )));
            }
            if same_fs && entry.is_dir() && dst_fs.is_within(&dest.path, &src.path) {
                return Err(Error::Invalid(format!(
                    "cannot put a folder inside itself: {}",
                    src.path
                )));
            }

            if is_move && same_fs {
                let exists = dst_fs.exists(&target).await?;
                if exists && conflict == ConflictPolicy::Fail {
                    return Err(Error::AlreadyExists(target));
                }
                if exists && conflict == ConflictPolicy::Skip {
                    self.prog().skipped += 1;
                    continue;
                }
                // Fast path: a rename. Falls back to copy + delete when it
                // fails (e.g. across volumes) or the target must be merged.
                if !exists && src_fs.rename(&src.path, &target).await.is_ok() {
                    self.prog().files_total += 1;
                    self.prog().files_done += 1;
                    self.set_current(&src.path);
                    continue;
                }
            }

            self.plan(&src_fs, &dst_fs, entry, target, &mut plan)
                .await?;
            if is_move {
                move_roots.push((src_fs, src.path.clone()));
            }
        }

        for item in &plan {
            if !item.dir {
                self.prog().files_total += 1;
                self.prog().bytes_total += item.size;
            }
        }
        self.emit(true);

        self.copy_all(&plan, &dst_fs, conflict, is_move).await?;

        // Remove now-empty source folders of a move, deepest first. Folders
        // that still hold skipped files stay where they are.
        if is_move {
            for item in plan.iter().rev().filter(|i| i.dir) {
                let _ = item.src_fs.remove_dir(&item.src).await;
            }
            for (fs, root) in move_roots {
                if !fs.exists(&root).await.unwrap_or(true) {
                    continue;
                }
                if !fs.stat(&root).await?.is_dir() {
                    continue;
                }
                let _ = fs.remove_dir(&root).await;
            }
        }
        Ok(())
    }

    /// Walk `entry` and append the folders (before their contents) and files
    /// to copy into `out`.
    async fn plan(
        &mut self,
        src_fs: &Fs,
        dst_fs: &Fs,
        root: Entry,
        root_dst: String,
        out: &mut Vec<Item>,
    ) -> Result<()> {
        let mut stack = vec![(root, root_dst)];
        while let Some((entry, dst)) = stack.pop() {
            self.check()?;
            match entry.kind {
                // Symlinked folders are not followed, to avoid cycles.
                EntryKind::Dir if !entry.symlink => {
                    let children = src_fs.list(&entry.path).await?;
                    out.push(Item {
                        src_fs: src_fs.clone(),
                        src: entry.path,
                        dst: dst.clone(),
                        dir: true,
                        size: 0,
                        modified_ms: entry.modified_ms,
                    });
                    for child in children.into_iter().rev() {
                        check_name(dst_fs, &child.name)?;
                        let child_dst = dst_fs.join(&dst, &child.name);
                        stack.push((child, child_dst));
                    }
                }
                EntryKind::File => out.push(Item {
                    src_fs: src_fs.clone(),
                    src: entry.path,
                    dst,
                    dir: false,
                    size: entry.size,
                    modified_ms: entry.modified_ms,
                }),
                _ => self.prog().skipped += 1,
            }
        }
        Ok(())
    }

    /// Create the folders of `plan`, then copy its files, several at a time
    /// when either side is remote storage. A move removes each source file
    /// once its copy is complete.
    async fn copy_all(
        &self,
        plan: &[Item],
        dst_fs: &Fs,
        conflict: ConflictPolicy,
        is_move: bool,
    ) -> Result<()> {
        for item in plan.iter().filter(|i| i.dir) {
            self.check()?;
            self.set_current(&item.src);
            make_dir(dst_fs, &item.dst).await?;
        }
        let files: Vec<&Item> = plan.iter().filter(|i| !i.dir).collect();
        let workers = files
            .iter()
            .map(|i| i.src_fs.parallel_transfers())
            .max()
            .unwrap_or(1)
            .max(dst_fs.parallel_transfers())
            .max(1);
        // Two files whose names differ only in case land on the same file on
        // most Mac and Windows disks; copy those one at a time, so that the
        // conflict rule decides between them.
        let mut seen = std::collections::HashSet::new();
        let workers = if files.iter().all(|i| seen.insert(i.dst.to_lowercase())) {
            workers
        } else {
            1
        };
        // Stops the other copies when one fails.
        let stop = self.cancel.child_token();
        let queue = Mutex::new(files.into_iter());
        let mut tasks = Vec::with_capacity(workers);
        for _ in 0..workers {
            tasks.push(self.copy_worker(&queue, dst_fs, conflict, is_move, &stop));
        }
        let results: Vec<Result<()>> = futures_util::future::join_all(tasks).await;
        self.check()?;
        // Report the failure that stopped the others, not their "cancelled".
        let mut errors = results.into_iter().filter_map(|r| r.err());
        let first = errors.next();
        match errors.find(|e| !matches!(e, Error::Cancelled)).or(first) {
            Some(Error::Cancelled) | None => Ok(()),
            Some(e) => Err(e),
        }
    }

    /// Copy files from `queue` until it is empty or a copy fails.
    async fn copy_worker(
        &self,
        queue: &Mutex<std::vec::IntoIter<&Item>>,
        dst_fs: &Fs,
        conflict: ConflictPolicy,
        is_move: bool,
        stop: &CancellationToken,
    ) -> Result<()> {
        loop {
            let Some(item) = queue.lock().unwrap().next() else {
                return Ok(());
            };
            let result = async {
                if self.copy_file(item, dst_fs, conflict, stop).await? && is_move {
                    item.src_fs.remove_file(&item.src).await?;
                }
                Ok(())
            }
            .await;
            if result.is_err() {
                stop.cancel();
                return result;
            }
        }
    }

    /// Copy one file. It is written under a temporary name and renamed into
    /// place once complete, so the target never holds half a file. A copy to
    /// or from remote storage that fails midway is retried a few times,
    /// carrying on from where it stopped when the target allows it.
    ///
    /// Returns true if the file was copied, false if it was skipped.
    async fn copy_file(
        &self,
        item: &Item,
        dst_fs: &Fs,
        conflict: ConflictPolicy,
        stop: &CancellationToken,
    ) -> Result<bool> {
        if stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.set_current(&item.src);
        let existing = match dst_fs.stat(&item.dst).await {
            Ok(e) => Some(e),
            Err(Error::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        if existing.is_some() {
            match conflict {
                ConflictPolicy::Fail => return Err(Error::AlreadyExists(item.dst.clone())),
                ConflictPolicy::Skip => {
                    let mut p = self.prog();
                    p.skipped += 1;
                    p.bytes_done += item.size;
                    return Ok(false);
                }
                ConflictPolicy::Overwrite => {}
            }
        }
        let part = if dst_fs.atomic_writes() {
            item.dst.clone()
        } else {
            part_path(dst_fs, &item.dst)
        };
        let retry = item.src_fs.id() != LocalFs::ID || dst_fs.id() != LocalFs::ID;
        let mut offset = 0; // bytes of this file already in place
        let mut attempt = 0;
        let mut created = false; // whether `part` is ours to remove
        let result = loop {
            let mut sent = offset;
            let tried = self
                .copy_attempt(item, dst_fs, &part, offset, (&mut sent, &mut created), stop)
                .await;
            let e = match tried {
                Ok(()) => break Ok(()),
                Err(e) => e,
            };
            // Take back the progress of bytes that will be sent again.
            let moved = sent > offset;
            let resume = if retry && attempt < RETRY_DELAYS.len() && is_transient(&e, moved) {
                let delay = RETRY_DELAYS[attempt];
                attempt += 1;
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = stop.cancelled() => {}
                }
                if stop.is_cancelled() {
                    None
                } else {
                    Some(resume_offset(dst_fs, &part, item.size).await)
                }
            } else {
                None
            };
            self.prog().bytes_done -= sent - resume.unwrap_or(sent).min(sent);
            match resume {
                Some(next) => offset = next,
                None => {
                    break Err(if stop.is_cancelled() {
                        Error::Cancelled
                    } else {
                        e
                    })
                }
            }
        };
        if let Err(e) = result {
            // Don't leave a partial file. An upload that only appears once
            // complete has left nothing, and the file at `part` is the
            // original there.
            if created && !dst_fs.atomic_writes() {
                let _ = dst_fs.remove_file(&part).await;
            }
            return Err(e);
        }
        if part != item.dst {
            // The new file replaces the old one, so give it the old one's
            // permissions, as writing into it would have kept them.
            if let Some(mode) = existing.and_then(|e| e.mode) {
                let _ = dst_fs.set_mode(&part, mode & 0o7777).await;
            }
            if let Err(e) = dst_fs.rename(&part, &item.dst).await {
                let _ = dst_fs.remove_file(&part).await;
                return Err(e);
            }
        }
        if let Some(ms) = item.modified_ms {
            let _ = dst_fs.set_modified(&item.dst, ms).await;
        }
        self.prog().files_done += 1;
        self.emit(false);
        Ok(true)
    }

    /// Copy `item` into `part` from `offset` on, adding each byte written to
    /// `sent` and to the progress, and setting `created` once `part` may exist.
    async fn copy_attempt(
        &self,
        item: &Item,
        dst_fs: &Fs,
        part: &str,
        offset: u64,
        (sent, created): (&mut u64, &mut bool),
        stop: &CancellationToken,
    ) -> Result<()> {
        let (mut reader, mut writer) = if offset > 0 {
            (
                item.src_fs.open_read_at(&item.src, offset).await?,
                dst_fs.open_write_at(part, offset).await?,
            )
        } else {
            let reader = item.src_fs.open_read(&item.src).await?;
            let writer = dst_fs.open_write(part).await;
            // Even a failed open may have created the file.
            *created = true;
            (reader, writer?)
        };
        let mut buf = vec![0u8; CHUNK];
        loop {
            if stop.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let n = reader
                .read(&mut buf)
                .await
                .map_err(|e| Error::io(&item.src, e))?;
            if n == 0 {
                break;
            }
            writer
                .write_all(&buf[..n])
                .await
                .map_err(|e| Error::io(part, e))?;
            *sent += n as u64;
            self.prog().bytes_done += n as u64;
            self.emit(false);
        }
        writer.flush().await.map_err(|e| Error::io(part, e))?;
        writer.shutdown().await.map_err(|e| Error::io(part, e))
    }

    // ---- trash -------------------------------------------------------------

    async fn trash(&mut self, targets: Vec<Location>) -> Result<()> {
        self.prog().files_total = targets.len() as u64;
        self.emit(true);
        for t in &targets {
            self.check()?;
            self.set_current(&t.path);
            self.fs(&t.fs)?.trash(&t.path).await?;
            self.prog().files_done += 1;
        }
        Ok(())
    }

    // ---- delete ------------------------------------------------------------

    async fn delete(&mut self, targets: Vec<Location>) -> Result<()> {
        // (fs, path, is_real_dir) in pre-order; deleted in reverse so that
        // folder contents go before the folder itself.
        let mut items: Vec<(Fs, String, bool)> = Vec::new();
        for t in &targets {
            let fs = self.fs(&t.fs)?;
            let root = fs.stat(&t.path).await?;
            let mut stack = vec![root];
            while let Some(e) = stack.pop() {
                self.check()?;
                let real_dir = e.is_dir() && !e.symlink;
                if real_dir {
                    stack.extend(fs.list(&e.path).await?);
                }
                items.push((fs.clone(), e.path, real_dir));
            }
        }
        self.prog().files_total = items.len() as u64;
        self.emit(true);
        for (fs, path, dir) in items.into_iter().rev() {
            self.check()?;
            self.set_current(&path);
            if dir {
                fs.remove_dir(&path).await?;
            } else {
                fs.remove_file(&path).await?;
            }
            self.prog().files_done += 1;
        }
        Ok(())
    }

    // ---- compress -----------------------------------------------------------

    async fn compress(&mut self, sources: Vec<Location>, dest: Location) -> Result<()> {
        use async_zip::base::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};
        use futures_lite::io::AsyncWriteExt as _;

        let dst_fs = self.fs(&dest.fs)?;
        if dst_fs.exists(&dest.path).await? {
            return Err(Error::AlreadyExists(dest.path.clone()));
        }

        // (source fs, source entry, path inside the archive)
        let mut items: Vec<(Fs, Entry, String)> = Vec::new();
        for src in &sources {
            let fs = self.fs(&src.fs)?;
            let root = fs.stat(&src.path).await?;
            let mut stack = vec![(root.clone(), root.name.clone())];
            while let Some((e, name)) = stack.pop() {
                self.check()?;
                match e.kind {
                    EntryKind::Dir if !e.symlink => {
                        for c in fs.list(&e.path).await?.into_iter().rev() {
                            let child = format!("{name}/{}", c.name);
                            stack.push((c, child));
                        }
                        items.push((fs.clone(), e, format!("{name}/")));
                    }
                    EntryKind::File => items.push((fs.clone(), e, name)),
                    _ => self.prog().skipped += 1,
                }
            }
        }
        for (_, e, _) in &items {
            if e.kind == EntryKind::File {
                self.prog().files_total += 1;
                self.prog().bytes_total += e.size;
            }
        }
        self.emit(true);

        let writer = dst_fs.open_write(&dest.path).await?;
        let result: Result<()> = async {
            let zip_err = |e: async_zip::error::ZipError| Error::Io {
                path: dest.path.clone(),
                source: std::io::Error::other(e),
            };
            let mut zip = ZipFileWriter::with_tokio(writer);
            for (fs, e, name) in &items {
                self.check()?;
                self.set_current(&e.path);
                let mut builder = ZipEntryBuilder::new(
                    name.clone().into(),
                    if e.is_dir() {
                        Compression::Stored
                    } else {
                        Compression::Deflate
                    },
                );
                if let Some(ms) = e.modified_ms {
                    builder = builder.last_modification_date(zip_date(ms));
                }
                if let Some(mode) = e.mode {
                    builder = builder.unix_permissions(mode as u16);
                }
                if e.is_dir() {
                    zip.write_entry_whole(builder, &[]).await.map_err(zip_err)?;
                    continue;
                }
                let mut reader = fs.open_read(&e.path).await?;
                let mut entry = zip.write_entry_stream(builder).await.map_err(zip_err)?;
                let mut buf = vec![0u8; CHUNK];
                loop {
                    self.check()?;
                    let n = reader
                        .read(&mut buf)
                        .await
                        .map_err(|err| Error::io(&e.path, err))?;
                    if n == 0 {
                        break;
                    }
                    entry
                        .write_all(&buf[..n])
                        .await
                        .map_err(|err| Error::io(&dest.path, err))?;
                    self.prog().bytes_done += n as u64;
                    self.emit(false);
                }
                entry.close().await.map_err(zip_err)?;
                self.prog().files_done += 1;
            }
            let mut inner = zip.close().await.map_err(zip_err)?.into_inner();
            inner
                .shutdown()
                .await
                .map_err(|err| Error::io(&dest.path, err))
        }
        .await;
        if result.is_err() {
            let _ = dst_fs.remove_file(&dest.path).await; // don't leave a broken archive
        }
        result
    }

    async fn set_mode(&mut self, loc: Location, mode: u32) -> Result<()> {
        self.prog().files_total = 1;
        self.set_current(&loc.path);
        self.fs(&loc.fs)?.set_mode(&loc.path, mode).await?;
        self.prog().files_done = 1;
        Ok(())
    }

    // ---- single-item operations ---------------------------------------------

    async fn create_dir(&mut self, loc: Location) -> Result<()> {
        let fs = self.fs(&loc.fs)?;
        self.prog().files_total = 1;
        self.set_current(&loc.path);
        fs.create_dir(&loc.path).await?;
        self.prog().files_done = 1;
        Ok(())
    }

    async fn rename(&mut self, loc: Location, new_name: String) -> Result<()> {
        if crate::rename::name_problem(&new_name).is_some() {
            return Err(Error::Invalid(format!("invalid name: {new_name:?}")));
        }
        let fs = self.fs(&loc.fs)?;
        let parent = fs
            .parent(&loc.path)
            .ok_or_else(|| Error::Invalid(format!("cannot rename {}", loc.path)))?;
        let target = fs.join(&parent, &new_name);
        // A case-only rename on a case-insensitive disk sees the target as
        // existing (it is the same file), so allow that case through.
        let case_only = target.to_lowercase() == loc.path.to_lowercase();
        if !case_only && fs.exists(&target).await? {
            return Err(Error::AlreadyExists(target));
        }
        self.prog().files_total = 1;
        self.set_current(&loc.path);
        fs.rename(&loc.path, &target).await?;
        self.prog().files_done = 1;
        Ok(())
    }
}

/// Waits before each new try of a copy that failed midway.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(10),
];

/// Written bytes this close to the end of a cut-off file are sent again:
/// SFTP writes are acknowledged out of order, so the last ones in flight
/// may have left gaps.
const RESUME_MARGIN: u64 = 4 * 1024 * 1024;

/// A dropped connection, or a read or write failing partway through a file,
/// which may work if tried again. A file that cannot even be opened, or a
/// missing file or denied permission, will not.
fn is_transient(e: &Error, moved: bool) -> bool {
    match e {
        Error::Connection(_) => true,
        Error::Io { .. } => moved,
        _ => false,
    }
}

/// Where to carry on with a copy into `part` that was cut off: just before
/// its end, or 0 to start over when the target cannot be written midway.
async fn resume_offset(dst_fs: &Fs, part: &str, size: u64) -> u64 {
    match dst_fs.stat(part).await {
        Ok(e) if dst_fs.resumable_writes() && !dst_fs.atomic_writes() && e.size <= size => {
            let keep = e.size.saturating_sub(RESUME_MARGIN);
            keep - keep % CHUNK as u64
        }
        _ => 0,
    }
}

/// The hidden name a file is written under until its copy is complete.
/// It is new each time, so it never belongs to another file.
fn part_path(fs: &Fs, dst: &str) -> String {
    let parent = fs.parent(dst).unwrap_or_default();
    let name = fs.file_name(dst).unwrap_or_default();
    let tag = &Uuid::new_v4().simple().to_string()[..12];
    // Keep within the usual 255-byte limit on a name.
    let part = if name.len() <= 200 {
        format!(".{name}.{tag}.dualpal-part")
    } else {
        format!(".dualpal-{tag}.part")
    };
    fs.join(&parent, &part)
}

async fn make_dir(fs: &Fs, path: &str) -> Result<()> {
    match fs.create_dir(path).await {
        Ok(()) => Ok(()),
        Err(Error::AlreadyExists(_)) if fs.stat(path).await?.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

/// Names from a server must not escape the local folder they are copied
/// into (e.g. `..` or, on Windows, `C:\\Users\\…`).
fn check_name(dst_fs: &Fs, name: &str) -> Result<()> {
    if dst_fs.id() == LocalFs::ID && !crate::vfs::is_safe_local_name(name) {
        return Err(Error::Invalid(format!(
            "unsafe file name for this computer: {name}"
        )));
    }
    Ok(())
}

/// ZIP timestamp. The format has no time zone and, by convention, stores
/// local time — which is what unzip tools show.
fn zip_date(ms: i64) -> async_zip::ZipDateTime {
    use chrono::{Datelike, TimeZone, Timelike};
    let t = chrono::Local
        .timestamp_millis_opt(ms)
        .earliest()
        .unwrap_or_default()
        .naive_local();
    async_zip::ZipDateTimeBuilder::new()
        .year(t.year().max(1980))
        .month(t.month())
        .day(t.day())
        .hour(t.hour())
        .minute(t.minute())
        .second(t.second())
        .build()
}

// ---- batch rename ---------------------------------------------------------------

impl Job {
    async fn batch_rename(&mut self, items: Vec<RenameItem>) -> Result<()> {
        // (fs, from, to) for every item whose name changes.
        let mut moves: Vec<(Fs, String, String)> = Vec::new();
        for it in items {
            if let Some(p) = crate::rename::name_problem(&it.new_name) {
                return Err(Error::Invalid(format!("{}: {p}", it.new_name)));
            }
            let fs = self.fs(&it.location.fs)?;
            check_name(&fs, &it.new_name)?;
            let parent = fs
                .parent(&it.location.path)
                .ok_or_else(|| Error::Invalid(format!("cannot rename {}", it.location.path)))?;
            let to = fs.join(&parent, &it.new_name);
            if to != it.location.path {
                moves.push((fs, it.location.path, to));
            }
        }
        // Compare without case: most Mac and Windows disks ignore it.
        let key = |fs: &Fs, p: &str| format!("{}\n{}", fs.id(), p.to_lowercase());
        let sources: std::collections::HashSet<String> =
            moves.iter().map(|(fs, from, _)| key(fs, from)).collect();
        let mut targets = std::collections::HashSet::new();
        let mut swaps = false;
        for (fs, _, to) in &moves {
            if !targets.insert(key(fs, to)) {
                return Err(Error::Invalid(format!("two items would be named {to}")));
            }
            if sources.contains(&key(fs, to)) {
                // Takes the name another item is leaving, or only changes case.
                swaps = true;
            } else if fs.exists(to).await? {
                return Err(Error::AlreadyExists(to.clone()));
            }
        }
        self.prog().files_total = moves.len() as u64;
        self.emit(true);
        if !swaps {
            for (fs, from, to) in &moves {
                self.check()?;
                self.set_current(from);
                fs.rename(from, to).await?;
                self.prog().files_done += 1;
            }
            return Ok(());
        }
        // Names are swapped: move everything to a temporary name first.
        let tag = &Uuid::new_v4().simple().to_string()[..8];
        let mut parked: Vec<(Fs, String, String, String)> = Vec::new();
        let mut result = Ok(());
        for (i, (fs, from, to)) in moves.iter().enumerate() {
            let parent = fs.parent(from).unwrap_or_default();
            let tmp = fs.join(&parent, &format!(".dualpal-rename-{tag}-{i}"));
            self.set_current(from);
            if let Err(e) = fs.rename(from, &tmp).await {
                result = Err(e);
                break;
            }
            parked.push((fs.clone(), from.clone(), tmp, to.clone()));
        }
        let mut placed = 0;
        if result.is_ok() {
            for (fs, _, tmp, to) in &parked {
                if let Err(e) = fs.rename(tmp, to).await {
                    result = Err(e);
                    break;
                }
                placed += 1;
                self.prog().files_done += 1;
                self.emit(false);
            }
        }
        if result.is_err() {
            // Put back what has not reached its new name.
            for (fs, from, tmp, _) in parked.iter().skip(placed) {
                let _ = fs.rename(tmp, from).await;
            }
        }
        result
    }
}

// ---- sync -------------------------------------------------------------------------

impl Job {
    fn sync_done(&self, d: SyncDone) {
        self.inner
            .sync_results
            .lock()
            .unwrap()
            .entry(self.id)
            .or_default()
            .push(d);
    }

    async fn sync(&mut self, left: Location, right: Location, steps: Vec<SyncStep>) -> Result<()> {
        let lfs = self.fs(&left.fs)?;
        let rfs = self.fs(&right.fs)?;
        for s in &steps {
            let t = match s.action {
                SyncAction::CopyToRight => s.left,
                SyncAction::CopyToLeft => s.right,
                _ => None,
            };
            match s.action {
                SyncAction::Skip => {}
                SyncAction::DeleteLeft | SyncAction::DeleteRight => self.prog().files_total += 1,
                _ => {
                    let t = t.unwrap_or_default();
                    self.prog().files_total += t.files;
                    self.prog().bytes_total += t.bytes;
                }
            }
        }
        self.emit(true);
        for s in &steps {
            self.check()?;
            let (lp, rp) = (
                compare::resolve(lfs.as_ref(), &left.path, &s.rel),
                compare::resolve(rfs.as_ref(), &right.path, &s.rel),
            );
            match s.action {
                SyncAction::Skip => {}
                SyncAction::CopyToRight => {
                    self.sync_copy(&lfs, &lp, &rfs, &right.path, &s.rel, true)
                        .await?
                }
                SyncAction::CopyToLeft => {
                    self.sync_copy(&rfs, &rp, &lfs, &left.path, &s.rel, false)
                        .await?
                }
                SyncAction::DeleteLeft | SyncAction::DeleteRight => {
                    let (fs, path) = if s.action == SyncAction::DeleteLeft {
                        (&lfs, &lp)
                    } else {
                        (&rfs, &rp)
                    };
                    self.set_current(path);
                    if fs.exists(path).await? {
                        self.remove_tree(fs, path).await?;
                    }
                    self.prog().files_done += 1;
                    self.sync_done(SyncDone::Deleted { rel: s.rel.clone() });
                }
            }
        }
        Ok(())
    }

    /// Delete a file or a whole folder.
    async fn remove_tree(&mut self, fs: &Fs, path: &str) -> Result<()> {
        let root = fs.stat(path).await?;
        let mut items = Vec::new();
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            self.check()?;
            let real_dir = e.is_dir() && !e.symlink;
            if real_dir {
                stack.extend(fs.list(&e.path).await?);
            }
            items.push((e.path, real_dir));
        }
        for (p, dir) in items.into_iter().rev() {
            self.check()?;
            if dir {
                fs.remove_dir(&p).await?;
            } else {
                fs.remove_file(&p).await?;
            }
        }
        Ok(())
    }

    /// Copy the item at `rel` (a file or a whole folder) from `src` to the
    /// same place below `dst_root`, replacing what is there.
    async fn sync_copy(
        &mut self,
        src_fs: &Fs,
        src: &str,
        dst_fs: &Fs,
        dst_root: &str,
        rel: &str,
        to_right: bool,
    ) -> Result<()> {
        let entry = src_fs.stat(src).await?;
        // Create missing parent folders.
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent = dst_root.to_string();
        for seg in &segs[..segs.len().saturating_sub(1)] {
            check_name(dst_fs, seg)?;
            parent = dst_fs.join(&parent, seg);
            if let Err(e) = dst_fs.create_dir(&parent).await {
                // Some servers report an existing folder as a generic failure.
                if !dst_fs.stat(&parent).await.is_ok_and(|e| e.is_dir()) {
                    return Err(e);
                }
            }
        }
        let name = segs.last().copied().unwrap_or_default();
        check_name(dst_fs, name)?;
        let dst = dst_fs.join(&parent, name);
        // A file where a folder goes (or the other way round) is replaced.
        if let Ok(existing) = dst_fs.stat(&dst).await {
            if existing.is_dir() != entry.is_dir() {
                self.remove_tree(dst_fs, &dst).await?;
            }
        }
        let mut plan = Vec::new();
        self.plan(src_fs, dst_fs, entry, dst.clone(), &mut plan)
            .await?;
        self.copy_all(&plan, dst_fs, ConflictPolicy::Overwrite, false)
            .await?;
        for item in &plan {
            self.check()?;
            // Record both sides as they are now, for the next sync.
            let suffix = item.dst[dst.len()..].replace('\\', "/");
            let item_rel = format!("{rel}{suffix}");
            let (a, b) = (src_fs.stat(&item.src).await?, dst_fs.stat(&item.dst).await?);
            let (a, b) = (side(&a), side(&b));
            let pair = if to_right {
                SnapPair { l: a, r: b }
            } else {
                SnapPair { l: b, r: a }
            };
            self.sync_done(SyncDone::Copied {
                rel: item_rel,
                pair,
            });
        }
        Ok(())
    }
}

fn side(e: &Entry) -> Side {
    Side {
        dir: e.is_dir(),
        size: if e.is_dir() { 0 } else { e.size },
        modified_ms: e.modified_ms,
    }
}

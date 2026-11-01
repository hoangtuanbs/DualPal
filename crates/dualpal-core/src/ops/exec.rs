//! Executors for each [`Operation`] kind.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{ConflictPolicy, Inner, Operation, Progress};
use crate::compare::{self, Side};
use crate::error::{Error, Result};
use crate::rename::RenameItem;
use crate::sync::{SnapPair, SyncAction, SyncDone, SyncStep};
use crate::vfs::{Entry, EntryKind, FileSystem, Location, Reader, Writer};

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
    progress: Progress,
    last_emit: Instant,
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
        progress: Progress::default(),
        last_emit: Instant::now(),
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

    fn emit(&mut self, force: bool) {
        if force || self.last_emit.elapsed() >= EMIT_INTERVAL {
            self.last_emit = Instant::now();
            let p = self.progress.clone();
            self.inner.update(self.id, |i| i.progress = p);
        }
    }

    fn set_current(&mut self, path: &str) {
        self.progress.current = Some(path.to_string());
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
                    self.progress.skipped += 1;
                    continue;
                }
                // Fast path: a rename. Falls back to copy + delete when it
                // fails (e.g. across volumes) or the target must be merged.
                if !exists && src_fs.rename(&src.path, &target).await.is_ok() {
                    self.progress.files_total += 1;
                    self.progress.files_done += 1;
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
                self.progress.files_total += 1;
                self.progress.bytes_total += item.size;
            }
        }
        self.emit(true);

        for item in &plan {
            self.check()?;
            self.set_current(&item.src);
            if item.dir {
                match dst_fs.create_dir(&item.dst).await {
                    Ok(()) => {}
                    Err(Error::AlreadyExists(_)) if dst_fs.stat(&item.dst).await?.is_dir() => {}
                    Err(e) => return Err(e),
                }
            } else if self.copy_file(item, &dst_fs, conflict).await? && is_move {
                item.src_fs.remove_file(&item.src).await?;
            }
        }

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
                _ => self.progress.skipped += 1,
            }
        }
        Ok(())
    }

    /// Returns true if the file was copied, false if it was skipped.
    async fn copy_file(
        &mut self,
        item: &Item,
        dst_fs: &Fs,
        conflict: ConflictPolicy,
    ) -> Result<bool> {
        if dst_fs.exists(&item.dst).await? {
            match conflict {
                ConflictPolicy::Fail => return Err(Error::AlreadyExists(item.dst.clone())),
                ConflictPolicy::Skip => {
                    self.progress.skipped += 1;
                    self.progress.bytes_done += item.size;
                    return Ok(false);
                }
                ConflictPolicy::Overwrite => {}
            }
        }
        let mut reader = item.src_fs.open_read(&item.src).await?;
        let mut writer = dst_fs.open_write(&item.dst).await?;
        if let Err(e) = self
            .pump(&mut reader, &mut writer, &item.src, &item.dst)
            .await
        {
            drop(writer);
            let _ = dst_fs.remove_file(&item.dst).await; // don't leave a partial file
            return Err(e);
        }
        if let Some(ms) = item.modified_ms {
            let _ = dst_fs.set_modified(&item.dst, ms).await;
        }
        self.progress.files_done += 1;
        Ok(true)
    }

    async fn pump(&mut self, r: &mut Reader, w: &mut Writer, src: &str, dst: &str) -> Result<()> {
        let mut buf = vec![0u8; CHUNK];
        loop {
            self.check()?;
            let n = r.read(&mut buf).await.map_err(|e| Error::io(src, e))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])
                .await
                .map_err(|e| Error::io(dst, e))?;
            self.progress.bytes_done += n as u64;
            self.emit(false);
        }
        w.flush().await.map_err(|e| Error::io(dst, e))?;
        w.shutdown().await.map_err(|e| Error::io(dst, e))
    }

    // ---- trash -------------------------------------------------------------

    async fn trash(&mut self, targets: Vec<Location>) -> Result<()> {
        self.progress.files_total = targets.len() as u64;
        self.emit(true);
        for t in &targets {
            self.check()?;
            self.set_current(&t.path);
            self.fs(&t.fs)?.trash(&t.path).await?;
            self.progress.files_done += 1;
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
        self.progress.files_total = items.len() as u64;
        self.emit(true);
        for (fs, path, dir) in items.into_iter().rev() {
            self.check()?;
            self.set_current(&path);
            if dir {
                fs.remove_dir(&path).await?;
            } else {
                fs.remove_file(&path).await?;
            }
            self.progress.files_done += 1;
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
                    _ => self.progress.skipped += 1,
                }
            }
        }
        for (_, e, _) in &items {
            if e.kind == EntryKind::File {
                self.progress.files_total += 1;
                self.progress.bytes_total += e.size;
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
                    self.progress.bytes_done += n as u64;
                    self.emit(false);
                }
                entry.close().await.map_err(zip_err)?;
                self.progress.files_done += 1;
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
        self.progress.files_total = 1;
        self.set_current(&loc.path);
        self.fs(&loc.fs)?.set_mode(&loc.path, mode).await?;
        self.progress.files_done = 1;
        Ok(())
    }

    // ---- single-item operations ---------------------------------------------

    async fn create_dir(&mut self, loc: Location) -> Result<()> {
        let fs = self.fs(&loc.fs)?;
        self.progress.files_total = 1;
        self.set_current(&loc.path);
        fs.create_dir(&loc.path).await?;
        self.progress.files_done = 1;
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
        self.progress.files_total = 1;
        self.set_current(&loc.path);
        fs.rename(&loc.path, &target).await?;
        self.progress.files_done = 1;
        Ok(())
    }
}

/// Names from a server must not escape the local folder they are copied
/// into (e.g. `..` or, on Windows, `C:\\Users\\…`).
fn check_name(dst_fs: &Fs, name: &str) -> Result<()> {
    if dst_fs.id() == crate::vfs::LocalFs::ID && !crate::vfs::is_safe_local_name(name) {
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
        self.progress.files_total = moves.len() as u64;
        self.emit(true);
        if !swaps {
            for (fs, from, to) in &moves {
                self.check()?;
                self.set_current(from);
                fs.rename(from, to).await?;
                self.progress.files_done += 1;
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
                self.progress.files_done += 1;
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
                SyncAction::DeleteLeft | SyncAction::DeleteRight => self.progress.files_total += 1,
                _ => {
                    let t = t.unwrap_or_default();
                    self.progress.files_total += t.files;
                    self.progress.bytes_total += t.bytes;
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
                    self.progress.files_done += 1;
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
        for item in &plan {
            self.check()?;
            self.set_current(&item.src);
            if item.dir {
                match dst_fs.create_dir(&item.dst).await {
                    Ok(()) => {}
                    Err(Error::AlreadyExists(_)) if dst_fs.stat(&item.dst).await?.is_dir() => {}
                    Err(e) => return Err(e),
                }
            } else {
                self.copy_file(item, dst_fs, ConflictPolicy::Overwrite)
                    .await?;
            }
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

//! Archives browsed as read-only folders: zip, tar, tar.gz/.tgz, tar.bz2,
//! tar.xz, tar.zst, 7z and rar, and single compressed files (.gz, .bz2, .xz,
//! .zst), which open as a folder holding the one file.
//!
//! [`open_archive`] reads the archive's table of contents once and returns an
//! [`ArchiveFs`], a [`FileSystem`] whose paths (`/`, `/docs/a.txt`) are the
//! entries inside it. Copy, preview, folder sizes, find and compare work on it
//! like on any other place; anything that would change it fails with a
//! read-only error.
//!
//! * zip: the central directory is read; each file is decompressed when it
//!   is read.
//! * tar: one pass over the headers records where each file's bytes are.
//!   A compressed tar is first unpacked to a plain tar in the archive's
//!   scratch folder, so that files can be read in any order.
//! * a single compressed file is unpacked the same way; if what comes out
//!   is a tar, it is shown as one, otherwise as a folder with that file.
//! * 7z: the header is read; a file is decoded from its block when read (in
//!   a solid archive the files before it in the block are decoded too).
//! * rar: the headers are listed with UnRAR; a file is unpacked to the
//!   reader chunk by chunk (see [`super::rar`]).
//!
//! An archive on a server, or inside another archive, is downloaded into the
//! scratch folder first. The scratch folder is removed when the last user of
//! the [`ArchiveFs`] (a pane, or a copy still reading from it) lets go.
//!
//! Safety: entry names are cleaned so that none can point outside the
//! archive (`../x` and `/etc/x` become `x` and `etc/x`), a file never yields
//! more bytes than the archive declares, zip entries whose data overlap (a
//! common zip bomb) are refused, more than [`BOMB_MIN_SIZE`] unpacked at over
//! [`MAX_RATIO`]:1 (a zip file, a compressed tar, a 7z block) is refused, and
//! unpacking a compressed tar stops before it fills the disk.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{is_safe_local_name, safe_local_name, Entry, EntryKind, FileSystem, LocalFs, Reader};
use super::{Location, Registry, Writer};
use crate::error::{Error, Result};
use crate::ops::{OperationQueue, OperationStatus};

/// Message for zip and 7z archives that need a password.
pub const ENCRYPTED: &str = "password-protected archives are not supported yet";

/// Prefix of the ids under which open archives are registered.
pub const ID_PREFIX: &str = "archive:";

/// A file whose declared size is above this, and more than
/// [`MAX_RATIO`] times its compressed size, is refused as a likely zip bomb.
const BOMB_MIN_SIZE: u64 = 1 << 30;
const MAX_RATIO: u64 = 1000;

/// Whether `size` bytes unpacked from `compressed` bytes look like a bomb:
/// over `min_size` and more than [`MAX_RATIO`] times bigger.
fn looks_like_bomb(size: u64, compressed: u64, min_size: u64) -> bool {
    size > min_size && size / compressed.max(1) > MAX_RATIO
}
/// Unpacking a compressed tar leaves at least this much space free.
const KEEP_FREE: u64 = 512 << 20;
const CHUNK: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZ,
    Rar,
    /// A single compressed file: `notes.txt.gz` holds `notes.txt`.
    Gz,
    Bz2,
    Xz,
    Zst,
}

impl Format {
    /// Whether the archive is one compressed stream, unpacked before use.
    fn is_compressed(self) -> bool {
        !matches!(
            self,
            Format::Zip | Format::Tar | Format::SevenZ | Format::Rar
        )
    }

    /// The same compression with a tar (`tar`) or a single file inside.
    fn with_tar(self, tar: bool) -> Format {
        use Format::*;
        match (self, tar) {
            (TarGz | Gz, true) => TarGz,
            (TarGz | Gz, false) => Gz,
            (TarBz2 | Bz2, true) => TarBz2,
            (TarBz2 | Bz2, false) => Bz2,
            (TarXz | Xz, true) => TarXz,
            (TarXz | Xz, false) => Xz,
            (TarZst | Zst, true) => TarZst,
            (TarZst | Zst, false) => Zst,
            (f, _) => f,
        }
    }
}

/// Archive extensions, longest first, with their format.
const EXTENSIONS: &[(&str, Format)] = &[
    (".tar.gz", Format::TarGz),
    (".tar.bz2", Format::TarBz2),
    (".tar.xz", Format::TarXz),
    (".tar.zst", Format::TarZst),
    (".tbz2", Format::TarBz2),
    (".tzst", Format::TarZst),
    (".tgz", Format::TarGz),
    (".tbz", Format::TarBz2),
    (".txz", Format::TarXz),
    (".zip", Format::Zip),
    (".tar", Format::Tar),
    (".bz2", Format::Bz2),
    (".zst", Format::Zst),
    (".7z", Format::SevenZ),
    (".rar", Format::Rar),
    (".gz", Format::Gz),
    (".xz", Format::Xz),
];

/// The extension of `name` that makes it an archive, and its format.
fn split_extension(name: &str) -> Option<(usize, Format)> {
    let n = name.to_lowercase();
    EXTENSIONS
        .iter()
        .find(|(e, _)| n.ends_with(e) && n.len() > e.len())
        .map(|&(e, f)| (e.len(), f))
}

/// The archive format a file name suggests, if it is one Dual Pal opens.
pub fn format_from_name(name: &str) -> Option<Format> {
    split_extension(name).map(|(_, f)| f)
}

/// Whether `name` is an archive that can be opened as a folder.
pub fn is_archive_name(name: &str) -> bool {
    format_from_name(name).is_some()
}

/// `name` without its archive extension: the folder an extraction creates.
pub fn archive_stem(name: &str) -> String {
    match split_extension(name) {
        Some((len, _)) if name.is_char_boundary(name.len() - len) => {
            name[..name.len() - len].to_string()
        }
        _ => name.to_string(),
    }
}

/// The format from the file's first bytes, falling back to its name (old
/// tar files have no magic number, and an empty one is all zeros).
fn detect(path: &Path, name: &str) -> io::Result<Option<Format>> {
    let mut head = Vec::with_capacity(512);
    File::open(path)?.take(512).read_to_end(&mut head)?;
    let by_name = format_from_name(name);
    // A compressed file holds a tar when its name says so; a single file
    // that turns out to be a tar is found after unpacking.
    let tar = by_name.is_some_and(|f| f.with_tar(true) == f);
    let found = if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Some(Format::Zip)
    } else if head.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Some(Format::SevenZ)
    } else if head.starts_with(b"Rar!\x1A\x07") {
        Some(Format::Rar)
    } else if head.starts_with(&[0x1f, 0x8b]) {
        Some(Format::TarGz.with_tar(tar))
    } else if head.starts_with(b"BZh") {
        Some(Format::TarBz2.with_tar(tar))
    } else if head.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0]) {
        Some(Format::TarXz.with_tar(tar))
    } else if head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Some(Format::TarZst.with_tar(tar))
    } else if head.len() >= 262 && &head[257..262] == b"ustar" {
        Some(Format::Tar)
    } else {
        None
    };
    Ok(found.or(by_name))
}

// ---- scratch folder --------------------------------------------------------

/// A folder for one open archive (its download or unpacked tar), removed
/// when dropped.
#[derive(Debug)]
struct Scratch(PathBuf);

impl Scratch {
    fn create(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path).map_err(|e| Error::io(path.to_string_lossy(), e))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- index -----------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Data {
    /// A folder, an empty file, or something that is not copied (a link).
    None,
    /// Index in the zip central directory, and the compressed size.
    Zip { index: usize, compressed: u64 },
    /// Offset of the bytes in the (plain) tar file.
    Tar { offset: u64 },
    /// Index in the 7z archive's file list.
    SevenZ { index: usize },
    /// Index in the RAR archive's headers, and the packed size.
    Rar { index: usize, packed: u64 },
}

#[derive(Clone, Debug)]
struct Node {
    kind: EntryKind,
    symlink: bool,
    size: u64,
    modified_ms: Option<i64>,
    mode: Option<u32>,
    data: Data,
}

impl Node {
    fn dir() -> Self {
        Self {
            kind: EntryKind::Dir,
            symlink: false,
            size: 0,
            modified_ms: None,
            mode: None,
            data: Data::None,
        }
    }
}

/// The tree of entries, by cleaned path (`/a/b`).
#[derive(Default)]
struct Index {
    nodes: HashMap<String, Node>,
    /// Child names of each folder, in archive order.
    children: HashMap<String, Vec<String>>,
}

impl Index {
    fn new() -> Self {
        let mut i = Self::default();
        i.nodes.insert("/".into(), Node::dir());
        i
    }

    /// Add an entry at `parts`, creating the folders above it. A later entry
    /// with the same path replaces the earlier one (as tar does).
    fn insert(&mut self, parts: &[String], node: Node) {
        let mut dir = "/".to_string();
        for (i, part) in parts.iter().enumerate() {
            let path = child_path(&dir, part);
            let last = i == parts.len() - 1;
            match self.nodes.get_mut(&path) {
                None => {
                    self.children
                        .entry(dir.clone())
                        .or_default()
                        .push(part.clone());
                    self.nodes
                        .insert(path.clone(), if last { node.clone() } else { Node::dir() });
                }
                Some(existing) => {
                    if last {
                        // A folder keeps its contents when a file of the
                        // same name follows; only its details change.
                        if existing.kind == EntryKind::Dir && node.kind != EntryKind::Dir {
                            continue;
                        }
                        *existing = node.clone();
                    } else if existing.kind != EntryKind::Dir {
                        // A file in the way of a folder becomes the folder.
                        *existing = Node::dir();
                    }
                }
            }
            dir = path;
        }
    }
}

fn child_path(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parts of an entry name that stay inside the archive: empty and `.`
/// parts are dropped, `..` goes up but never above the top, a leading `/` or
/// drive letter is ignored, and characters that cannot be in a local file
/// name are replaced. `backslash`: `\` separates folders too (zip, 7z).
fn clean_parts(raw: &str, backslash: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(|c| c == '/' || (backslash && c == '\\')) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p if out.is_empty()
                && p.len() == 2
                && p.ends_with(':')
                && p.as_bytes()[0].is_ascii_alphabetic() => {}
            p if is_safe_local_name(p) => out.push(p.to_string()),
            p => out.push(safe_local_name(p)),
        }
    }
    out
}

fn ms_from_system(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn damaged(name: &str, e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!(
        "{name} is damaged or not a supported archive ({e})"
    ))
}

fn index_zip(path: &Path, name: &str) -> Result<Index> {
    let file = File::open(path).map_err(|e| Error::io(path.to_string_lossy(), e))?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| damaged(name, e))?;
    let mut index = Index::new();
    // Each file with data.
    let mut with_data = Vec::new();
    for i in 0..za.len() {
        let e = za.by_index_data(i).map_err(|e| damaged(name, e))?;
        if e.encrypted() {
            return Err(Error::Unsupported(ENCRYPTED.into()));
        }
        let raw = e.name().map_err(|e| damaged(name, e))?.into_owned();
        let parts = clean_parts(&raw, true);
        if parts.is_empty() {
            continue;
        }
        let modified_ms = e.last_modified().and_then(|d| {
            use chrono::TimeZone;
            let date =
                chrono::NaiveDate::from_ymd_opt(d.year().into(), d.month().into(), d.day().into())?;
            let t = date.and_hms_opt(d.hour().into(), d.minute().into(), d.second().into())?;
            chrono::Local
                .from_local_datetime(&t)
                .earliest()
                .map(|t| t.timestamp_millis())
        });
        let mode = e.unix_mode().map(|m| m & 0o7777);
        let node = if e.is_dir() || raw.ends_with('/') || raw.ends_with('\\') {
            Node {
                modified_ms,
                mode,
                ..Node::dir()
            }
        } else {
            let symlink = e.is_symlink();
            if !symlink && e.compressed_size() > 0 {
                with_data.push(i);
            }
            Node {
                kind: if symlink {
                    EntryKind::Other
                } else {
                    EntryKind::File
                },
                symlink,
                size: if symlink { 0 } else { e.size() },
                modified_ms,
                mode,
                data: if symlink {
                    Data::None
                } else {
                    Data::Zip {
                        index: i,
                        compressed: e.compressed_size(),
                    }
                },
            }
        };
        index.insert(&parts, node);
    }
    // Entries whose data run into the next entry's header share bytes: the
    // trick behind overlapping-file zip bombs. The data starts after the
    // local header's name and extra field, so it is looked up.
    let mut spans = Vec::new();
    for i in with_data {
        let e = za.by_index_raw(i).map_err(|e| damaged(name, e))?;
        let start = e.header_start();
        let data = e.data_start().unwrap_or(start.saturating_add(30));
        spans.push((start, data.saturating_add(e.compressed_size())));
    }
    spans.sort_unstable();
    for w in spans.windows(2) {
        if w[0].1 > w[1].0 {
            return Err(Error::Invalid(format!(
                "{name} has overlapping entries and may be a zip bomb, so it is not opened"
            )));
        }
    }
    Ok(index)
}

fn index_tar(path: &Path, name: &str) -> Result<Index> {
    use tar::EntryType;
    let file = File::open(path).map_err(|e| Error::io(path.to_string_lossy(), e))?;
    let mut ar = tar::Archive::new(file);
    let mut index = Index::new();
    let entries = ar.entries_with_seek().map_err(|e| damaged(name, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| damaged(name, e))?;
        let header = entry.header();
        let raw = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let parts = clean_parts(&raw, false);
        if parts.is_empty() {
            continue;
        }
        let modified_ms = header.mtime().ok().map(|s| s as i64 * 1000);
        let mode = header.mode().ok().map(|m| m & 0o7777);
        let kind = header.entry_type();
        let node = match kind {
            EntryType::Directory => Node {
                modified_ms,
                mode,
                ..Node::dir()
            },
            EntryType::Regular | EntryType::Continuous => Node {
                kind: EntryKind::File,
                symlink: false,
                size: entry.size(),
                modified_ms,
                mode,
                data: if entry.size() == 0 {
                    Data::None
                } else {
                    Data::Tar {
                        offset: entry.raw_file_position(),
                    }
                },
            },
            // A hard link reads as the file it links to, when that came first.
            EntryType::Link => {
                let target = entry
                    .link_name_bytes()
                    .map(|b| clean_parts(&String::from_utf8_lossy(&b), false))
                    .unwrap_or_default();
                let target = format!("/{}", target.join("/"));
                match index.nodes.get(&target) {
                    Some(t) if t.kind == EntryKind::File => Node {
                        modified_ms,
                        mode,
                        ..t.clone()
                    },
                    _ => continue,
                }
            }
            EntryType::XGlobalHeader | EntryType::XHeader => continue,
            EntryType::GNULongName | EntryType::GNULongLink => continue,
            // Symbolic links, devices, fifos and sparse files are listed
            // but not copied.
            _ => Node {
                kind: EntryKind::Other,
                symlink: kind == EntryType::Symlink,
                size: 0,
                modified_ms,
                mode,
                data: Data::None,
            },
        };
        index.insert(&parts, node);
    }
    Ok(index)
}

fn seven_error(name: &str, e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired | E::MaybeBadPassword(_) => Error::Unsupported(ENCRYPTED.into()),
        e => damaged(name, e),
    }
}

fn index_7z(path: &Path, name: &str) -> Result<(Index, sevenz_rust2::Archive)> {
    let mut file = File::open(path).map_err(|e| Error::io(path.to_string_lossy(), e))?;
    let archive = sevenz_rust2::Archive::read(&mut file, &sevenz_rust2::Password::empty())
        .map_err(|e| seven_error(name, e))?;
    let mut index = Index::new();
    for (i, e) in archive.files.iter().enumerate() {
        if archive.is_file_encrypted(i) {
            return Err(Error::Unsupported(ENCRYPTED.into()));
        }
        if e.is_anti_item() {
            continue;
        }
        let parts = clean_parts(e.name(), true);
        if parts.is_empty() {
            continue;
        }
        let modified_ms = if e.has_last_modified_date {
            ms_from_system(e.last_modified_date().into())
        } else {
            None
        };
        // The high 16 bits of the attributes hold Unix permissions when bit 15 is set.
        let mode = (e.has_windows_attributes && e.windows_attributes & 0x8000 != 0)
            .then_some((e.windows_attributes >> 16) & 0o7777);
        let node = if e.is_directory() {
            Node {
                modified_ms,
                mode,
                ..Node::dir()
            }
        } else {
            Node {
                kind: EntryKind::File,
                symlink: false,
                size: e.size(),
                modified_ms,
                mode,
                data: if e.has_stream() {
                    Data::SevenZ { index: i }
                } else {
                    Data::None
                },
            }
        };
        index.insert(&parts, node);
    }
    Ok((index, archive))
}

fn rar_error(name: &str, e: super::rar::RarError) -> Error {
    match e {
        super::rar::RarError::Password => Error::Unsupported(ENCRYPTED.into()),
        e if e.is_missing_volume() => Error::Unsupported(format!(
            "{name} is part of a multi-volume RAR set, which is not supported yet"
        )),
        e => damaged(name, e),
    }
}

fn index_rar(path: &Path, name: &str) -> Result<Index> {
    let entries = super::rar::list(path).map_err(|e| rar_error(name, e))?;
    let mut index = Index::new();
    for (i, e) in entries.iter().enumerate() {
        if e.encrypted {
            return Err(Error::Unsupported(ENCRYPTED.into()));
        }
        if e.split {
            return Err(Error::Unsupported(format!(
                "{name} is part of a multi-volume RAR set, which is not supported yet"
            )));
        }
        let parts = clean_parts(&e.name, true);
        if parts.is_empty() {
            continue;
        }
        let node = if e.dir {
            Node {
                modified_ms: e.modified_ms,
                ..Node::dir()
            }
        } else {
            Node {
                kind: EntryKind::File,
                symlink: false,
                size: e.size,
                modified_ms: e.modified_ms,
                mode: None,
                data: if e.size > 0 {
                    Data::Rar {
                        index: i,
                        packed: e.packed,
                    }
                } else {
                    Data::None
                },
            }
        };
        index.insert(&parts, node);
    }
    Ok(index)
}

/// Whether the file at `path` starts with a tar header (`ustar` magic).
fn looks_like_tar(path: &Path) -> bool {
    let mut head = [0u8; 262];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head[257..262] == b"ustar"
}

/// The index of a single compressed file: one entry, named like the
/// archive without its extension, whose bytes are the whole of `unpacked`.
fn index_single(unpacked: &Path, name: &str, modified_ms: Option<i64>) -> Index {
    let size = std::fs::metadata(unpacked).map(|m| m.len()).unwrap_or(0);
    let mut inner = archive_stem(name);
    if inner.is_empty() || inner == name {
        inner = "data".into();
    }
    let parts = clean_parts(&inner, false);
    let mut index = Index::new();
    index.insert(
        &parts,
        Node {
            kind: EntryKind::File,
            symlink: false,
            size,
            modified_ms,
            mode: None,
            data: if size > 0 {
                Data::Tar { offset: 0 }
            } else {
                Data::None
            },
        },
    );
    index
}

/// Zstandard decoding of every frame in a stream (`zstd` writes one per
/// file; concatenated files hold several).
struct ZstdFrames<R: Read> {
    decoder: Option<
        ruzstd::decoding::StreamingDecoder<io::BufReader<R>, ruzstd::decoding::FrameDecoder>,
    >,
    source: Option<io::BufReader<R>>,
}

impl<R: Read> ZstdFrames<R> {
    fn new(inner: R) -> Self {
        Self {
            decoder: None,
            source: Some(io::BufReader::new(inner)),
        }
    }
}

impl<R: Read> Read for ZstdFrames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use io::BufRead;
        loop {
            if let Some(d) = self.decoder.as_mut() {
                let n = d.read(buf)?;
                if n > 0 || buf.is_empty() {
                    return Ok(n);
                }
                self.source = self.decoder.take().map(|d| d.into_inner());
            }
            let Some(mut src) = self.source.take() else {
                return Ok(0);
            };
            if src.fill_buf()?.is_empty() {
                return Ok(0);
            }
            let d = ruzstd::decoding::StreamingDecoder::new(src)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            self.decoder = Some(d);
        }
    }
}

/// Unpack the compressed tar `src` into the plain tar `dst`, reporting
/// (compressed bytes read, compressed total).
fn unpack_tar(
    format: Format,
    src: &Path,
    dst: &Path,
    name: &str,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64, u64) + Sync),
    bomb_min: u64,
) -> Result<()> {
    let file = File::open(src).map_err(|e| Error::io(src.to_string_lossy(), e))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let read = Arc::new(AtomicU64::new(0));
    let counted = Counted {
        inner: file,
        count: read.clone(),
    };
    let mut decoder: Box<dyn Read> = match format {
        Format::TarGz | Format::Gz => Box::new(flate2::read::MultiGzDecoder::new(counted)),
        Format::TarBz2 | Format::Bz2 => Box::new(bzip2::read::MultiBzDecoder::new(counted)),
        Format::TarXz | Format::Xz => Box::new(lzma_rust2::XzReader::new(counted, true)),
        Format::TarZst | Format::Zst => Box::new(ZstdFrames::new(counted)),
        _ => return Err(Error::Invalid(format!("{name} is not compressed"))),
    };
    let dir = dst.parent().unwrap_or(dst).to_string_lossy().into_owned();
    let limit = crate::places::available_space(&dir)
        .map(|free| free.saturating_sub(KEEP_FREE))
        .unwrap_or(u64::MAX);
    let mut out = File::create(dst).map_err(|e| Error::io(dst.to_string_lossy(), e))?;
    let mut buf = vec![0u8; CHUNK];
    let mut written = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let n = match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(damaged(name, e)),
        };
        written += n as u64;
        if written > limit {
            return Err(Error::Invalid(format!(
                "{name} unpacks to more than the free disk space, so it is not opened"
            )));
        }
        if looks_like_bomb(written, read.load(Ordering::Relaxed), bomb_min) {
            return Err(Error::Invalid(format!(
                "{name} unpacks to over {MAX_RATIO} times its size and may be a zip bomb, so it is not opened"
            )));
        }
        out.write_all(&buf[..n])
            .map_err(|e| Error::io(dst.to_string_lossy(), e))?;
        progress(read.load(Ordering::Relaxed), total);
    }
    Ok(())
}

struct Counted<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

// ---- reading files ---------------------------------------------------------

enum Msg {
    Data(Vec<u8>),
    Done,
    Fail(io::Error),
}

/// The pane-facing end of a file being read on a blocking thread.
struct ChannelReader {
    rx: mpsc::Receiver<Msg>,
    buf: Vec<u8>,
    pos: usize,
    done: bool,
}

impl AsyncRead for ChannelReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.pos < this.buf.len() {
                let n = (this.buf.len() - this.pos).min(out.remaining());
                out.put_slice(&this.buf[this.pos..this.pos + n]);
                this.pos += n;
                return Poll::Ready(Ok(()));
            }
            if this.done {
                return Poll::Ready(Ok(()));
            }
            match this.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Msg::Data(d))) => {
                    this.buf = d;
                    this.pos = 0;
                }
                Poll::Ready(Some(Msg::Done)) => this.done = true,
                Poll::Ready(Some(Msg::Fail(e))) => {
                    this.done = true;
                    return Poll::Ready(Err(e));
                }
                // The reading thread ended without saying it finished.
                Poll::Ready(None) => {
                    this.done = true;
                    return Poll::Ready(Err(io::Error::other("reading the archive stopped")));
                }
            }
        }
    }
}

/// Sends what is written to a [`ChannelReader`], and fails once more than
/// `limit` bytes arrive, so that a file can never be bigger than the archive
/// says it is.
struct ChannelWriter {
    tx: mpsc::Sender<Msg>,
    buf: Vec<u8>,
    written: u64,
    limit: u64,
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.written += data.len() as u64;
        if self.written > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the file holds more data than the archive says; it may be damaged or a zip bomb",
            ));
        }
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.flush()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK));
        self.tx
            .blocking_send(Msg::Data(chunk))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "reader closed"))
    }
}

/// Run `produce` on a blocking thread and stream what it writes. The stream
/// fails unless exactly `size` bytes arrive.
fn stream<F>(size: u64, produce: F) -> Reader
where
    F: FnOnce(&mut ChannelWriter) -> io::Result<()> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        let mut w = ChannelWriter {
            tx: tx.clone(),
            buf: Vec::with_capacity(CHUNK),
            written: 0,
            limit: size,
        };
        let result = produce(&mut w).and_then(|()| w.flush()).and_then(|()| {
            if w.written < size {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the file is shorter than the archive says; it may be damaged",
                ))
            } else {
                Ok(())
            }
        });
        let _ = tx.blocking_send(match result {
            Ok(()) => Msg::Done,
            Err(e) => Msg::Fail(e),
        });
    });
    Box::new(ChannelReader {
        rx,
        buf: Vec::new(),
        pos: 0,
        done: false,
    })
}

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

// ---- the file system -------------------------------------------------------

enum Backing {
    Zip {
        /// Opened archives to reuse, so the central directory is read once.
        pool: Mutex<Vec<zip::ZipArchive<File>>>,
    },
    Tar,
    SevenZ(Box<sevenz_rust2::Archive>),
}

struct Inner {
    /// The file the entries are read from: the archive, its download, or
    /// the unpacked tar.
    file: PathBuf,
    backing: Backing,
    index: Index,
    /// Files bigger than this are refused when they look like a bomb.
    bomb_min: u64,
    /// Held by every read, so the scratch folder outlives them.
    _scratch: Arc<Scratch>,
}

/// An open archive: a read-only [`FileSystem`] over its entries.
pub struct ArchiveFs {
    id: String,
    /// The archive's file name, e.g. `photos.zip`.
    name: String,
    /// Where the archive itself is.
    source: Location,
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ArchiveFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveFs")
            .field("id", &self.id)
            .field("source", &self.source)
            .finish()
    }
}

/// What [`open_archive`] is doing: `"download"` (an archive on a server),
/// or `"unpack"` (a compressed tar), with bytes done and the total.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProgress {
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
}

pub type ProgressFn = Arc<dyn Fn(OpenProgress) + Send + Sync>;

impl ArchiveFs {
    /// Open the archive file `file` (on the local disk) under the id `id`.
    fn open_blocking(
        id: String,
        file: PathBuf,
        name: String,
        source: Location,
        scratch: Scratch,
        cancel: &CancellationToken,
        progress: &ProgressFn,
    ) -> Result<Self> {
        let bomb_min = BOMB_MIN_SIZE;
        let format = detect(&file, &name)
            .map_err(|e| Error::io(file.to_string_lossy(), e))?
            .ok_or_else(|| Error::Unsupported(format!("opening {name} as an archive")))?;
        let (file, backing, index) = match format {
            Format::Zip => {
                let index = index_zip(&file, &name)?;
                let pool = Mutex::new(Vec::new());
                (file, Backing::Zip { pool }, index)
            }
            Format::Tar => {
                let index = index_tar(&file, &name)?;
                (file, Backing::Tar, index)
            }
            f if f.is_compressed() => {
                let tar = scratch.0.join("unpacked.tar");
                let modified_ms = std::fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64);
                let report = |done, total| {
                    progress(OpenProgress {
                        phase: "unpack",
                        done,
                        total,
                    })
                };
                unpack_tar(format, &file, &tar, &name, cancel, &report, bomb_min)?;
                // The download is not needed once unpacked.
                if file.starts_with(&scratch.0) {
                    let _ = std::fs::remove_file(&file);
                }
                let is_tar = f.with_tar(true) == f || looks_like_tar(&tar);
                let index = if is_tar {
                    index_tar(&tar, &name)?
                } else {
                    index_single(&tar, &name, modified_ms)
                };
                (tar, Backing::Tar, index)
            }
            Format::SevenZ => {
                let (index, archive) = index_7z(&file, &name)?;
                (file, Backing::SevenZ(Box::new(archive)), index)
            }
            Format::Rar => {
                let index = index_rar(&file, &name)?;
                (file, Backing::Tar, index)
            }
            _ => unreachable!("every format is handled above"),
        };
        Ok(Self {
            id,
            name,
            source,
            inner: Arc::new(Inner {
                file,
                backing,
                index,
                bomb_min,
                _scratch: Arc::new(scratch),
            }),
        })
    }

    /// Where the archive file itself is.
    pub fn source(&self) -> &Location {
        &self.source
    }

    /// The archive's file name.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn norm(path: &str) -> String {
        let p = path.trim_end_matches('/');
        if p.is_empty() {
            "/".into()
        } else if p.starts_with('/') {
            p.into()
        } else {
            format!("/{p}")
        }
    }

    fn node(&self, path: &str) -> Result<(String, &Node)> {
        let p = Self::norm(path);
        match self.inner.index.nodes.get(&p) {
            Some(n) => Ok((p, n)),
            None => Err(Error::NotFound(format!("{}{}", self.name, p))),
        }
    }

    fn entry(&self, path: &str, node: &Node) -> Entry {
        let name = if path == "/" {
            archive_stem(&self.name)
        } else {
            path.rsplit('/').next().unwrap_or_default().to_string()
        };
        Entry {
            hidden: name.starts_with('.'),
            name,
            path: path.to_string(),
            kind: node.kind,
            symlink: node.symlink,
            size: node.size,
            modified_ms: node.modified_ms,
            created_ms: None,
            readonly: true,
            mode: node.mode,
        }
    }

    fn bomb(&self, path: &str) -> Error {
        Error::Invalid(format!(
            "{}{path} unpacks to over {MAX_RATIO} times its size and may be a zip bomb, so it is not extracted",
            self.name
        ))
    }

    fn read_only(&self, path: &str) -> Error {
        Error::PermissionDenied(format!(
            "{}{} is inside an archive, which is read-only",
            self.name,
            Self::norm(path)
        ))
    }
}

#[async_trait]
impl FileSystem for ArchiveFs {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> String {
        self.name.clone()
    }

    fn join(&self, dir: &str, name: &str) -> String {
        child_path(&Self::norm(dir), name)
    }

    fn parent(&self, path: &str) -> Option<String> {
        let p = Self::norm(path);
        if p == "/" {
            return None;
        }
        let (head, _) = p.rsplit_once('/')?;
        Some(if head.is_empty() {
            "/".into()
        } else {
            head.into()
        })
    }

    /// The top of the archive is named after it (`photos` for
    /// `photos.zip`), so copying it out makes a folder of that name.
    fn file_name(&self, path: &str) -> Option<String> {
        let p = Self::norm(path);
        if p == "/" {
            Some(archive_stem(&self.name))
        } else {
            p.rsplit('/').next().map(str::to_string)
        }
    }

    fn is_within(&self, path: &str, ancestor: &str) -> bool {
        let (p, a) = (Self::norm(path), Self::norm(ancestor));
        a == "/" || p == a || p.starts_with(&format!("{a}/"))
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let (dir, node) = self.node(dir)?;
        if node.kind != EntryKind::Dir {
            return Err(Error::Invalid(format!("not a folder: {dir}")));
        }
        let names = self.inner.index.children.get(&dir);
        Ok(names
            .into_iter()
            .flatten()
            .filter_map(|n| {
                let path = child_path(&dir, n);
                let node = self.inner.index.nodes.get(&path)?;
                Some(self.entry(&path, node))
            })
            .collect())
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        let (p, node) = self.node(path)?;
        Ok(self.entry(&p, node))
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        Err(self.read_only(path))
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        Err(self.read_only(path))
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        Err(self.read_only(path))
    }

    async fn rename(&self, from: &str, _to: &str) -> Result<()> {
        Err(self.read_only(from))
    }

    async fn trash(&self, path: &str) -> Result<()> {
        Err(self.read_only(path))
    }

    async fn set_mode(&self, path: &str, _mode: u32) -> Result<()> {
        Err(self.read_only(path))
    }

    async fn open_write(&self, path: &str) -> Result<Writer> {
        Err(self.read_only(path))
    }

    async fn open_read(&self, path: &str) -> Result<Reader> {
        let (p, node) = self.node(path)?;
        if node.kind != EntryKind::File {
            return Err(Error::Invalid(format!("not a file: {}{p}", self.name)));
        }
        let size = node.size;
        let inner = self.inner.clone();
        Ok(match node.data {
            Data::None => stream(0, |_| Ok(())),
            Data::Tar { offset } => stream(size, move |w| {
                let mut f = File::open(&inner.file)?;
                f.seek(SeekFrom::Start(offset))?;
                io::copy(&mut f.take(size), w)?;
                Ok(())
            }),
            Data::Zip { index, compressed } => {
                if looks_like_bomb(size, compressed, inner.bomb_min) {
                    return Err(self.bomb(&p));
                }
                stream(size, move |w| {
                    let Backing::Zip { pool } = &inner.backing else {
                        return Err(io::Error::other("not a zip archive"));
                    };
                    let taken = pool.lock().unwrap().pop();
                    let mut za = match taken {
                        Some(za) => za,
                        None => zip::ZipArchive::new(File::open(&inner.file)?).map_err(to_io)?,
                    };
                    {
                        let mut f = za.by_index(index).map_err(to_io)?;
                        io::copy(&mut f, w)?;
                    }
                    pool.lock().unwrap().push(za);
                    Ok(())
                })
            }
            Data::SevenZ { index } => {
                let Backing::SevenZ(archive) = &inner.backing else {
                    return Err(Error::Invalid("not a 7z archive".into()));
                };
                let block = archive
                    .stream_map
                    .file_block_index
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or_else(|| Error::Invalid(format!("{}{p} has no data block", self.name)))?;
                // Reading a file decodes its block up to it, so the whole
                // block is checked.
                let first = archive.stream_map.block_first_pack_stream_index();
                let packs = archive.pack_sizes();
                let start = first.get(block).copied().unwrap_or(packs.len());
                let end = first.get(block + 1).copied().unwrap_or(packs.len());
                let packed: u64 = packs.get(start..end).unwrap_or_default().iter().sum();
                let unpacked = archive.blocks[block].get_unpack_size();
                if looks_like_bomb(unpacked, packed, inner.bomb_min) {
                    return Err(self.bomb(&p));
                }
                stream(size, move |w| seven_read(&inner, index, block, w))
            }
            Data::Rar { index, packed } => {
                if looks_like_bomb(size, packed, inner.bomb_min) {
                    return Err(self.bomb(&p));
                }
                let name = self.name.clone();
                stream(size, move |w| {
                    let mut failed = None;
                    let mut sink = |bytes: &[u8]| match w.write_all(bytes) {
                        Ok(()) => true,
                        Err(e) => {
                            failed = Some(e);
                            false
                        }
                    };
                    let result = super::rar::read(&inner.file, index, &mut sink);
                    if let Some(e) = failed {
                        return Err(e);
                    }
                    result.map_err(|e| io::Error::other(rar_error(&name, e).to_string()))
                })
            }
        })
    }
}

/// Write the 7z file `index`, which is in `block`, to `w`.
fn seven_read(inner: &Inner, index: usize, block: usize, w: &mut ChannelWriter) -> io::Result<()> {
    let Backing::SevenZ(archive) = &inner.backing else {
        return Err(io::Error::other("not a 7z archive"));
    };
    let target: *const sevenz_rust2::ArchiveEntry = &archive.files[index];
    let mut f = File::open(&inner.file)?;
    let password = sevenz_rust2::Password::empty();
    let decoder = sevenz_rust2::BlockDecoder::new(1, block, archive, &password, &mut f);
    let mut found = false;
    decoder
        .for_each_entries(&mut |entry, r| {
            if std::ptr::eq(entry, target) {
                found = true;
                io::copy(r, w)?;
                Ok(false)
            } else {
                Ok(true)
            }
        })
        .map_err(to_io)?;
    if !found {
        return Err(io::Error::other("the file is missing from its block"));
    }
    Ok(())
}

// ---- opening and closing ---------------------------------------------------

/// Open the archive at `location` (on any registered file system). An
/// archive that is not on the local disk is downloaded into a folder below
/// `temp_root` first. `id` is the id to give the new file system.
pub async fn open_archive(
    registry: &Registry,
    location: &Location,
    id: String,
    temp_root: &Path,
    cancel: &CancellationToken,
    progress: ProgressFn,
) -> Result<ArchiveFs> {
    let src_fs = registry.get(&location.fs)?;
    let name = src_fs
        .file_name(&location.path)
        .ok_or_else(|| Error::Invalid(format!("not a file: {}", location.path)))?;
    let entry = src_fs.stat(&location.path).await?;
    if entry.kind != EntryKind::File {
        return Err(Error::Invalid(format!("not a file: {}", location.path)));
    }
    let scratch = Scratch::create(temp_root.join(id.trim_start_matches(ID_PREFIX)))?;
    let file = if location.fs == LocalFs::ID {
        PathBuf::from(&location.path)
    } else {
        let dst = scratch.0.join("download");
        download(
            src_fs.as_ref(),
            &location.path,
            &dst,
            entry.size,
            cancel,
            &progress,
        )
        .await?;
        dst
    };
    let cancel = cancel.clone();
    let source = location.clone();
    tokio::task::spawn_blocking(move || {
        ArchiveFs::open_blocking(id, file, name, source, scratch, &cancel, &progress)
    })
    .await
    .map_err(|e| Error::Io {
        path: location.path.clone(),
        source: io::Error::other(e),
    })?
}

async fn download(
    fs: &dyn FileSystem,
    path: &str,
    dst: &Path,
    total: u64,
    cancel: &CancellationToken,
    progress: &ProgressFn,
) -> Result<()> {
    let mut reader = fs.open_read(path).await?;
    let dst_name = dst.to_string_lossy().into_owned();
    let mut out = tokio::fs::File::create(dst)
        .await
        .map_err(|e| Error::io(&dst_name, e))?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = tokio::select! {
            r = reader.read(&mut buf) => r.map_err(|e| Error::io(path, e))?,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .await
            .map_err(|e| Error::io(&dst_name, e))?;
        done += n as u64;
        progress(OpenProgress {
            phase: "download",
            done,
            total,
        });
    }
    out.flush().await.map_err(|e| Error::io(&dst_name, e))
}

/// The archives open in the app, registered in its [`Registry`].
///
/// An archive the panes no longer show is closed with [`Archives::close`].
/// When a queued or running operation still uses it (an extraction), it
/// stays registered until that operation ends and [`Archives::reap`] runs.
/// An extraction that stopped on an existing file keeps it until the
/// finished operations are cleared, so that it can be retried.
pub struct Archives {
    registry: Arc<Registry>,
    queue: OperationQueue,
    dir: PathBuf,
    open: Mutex<HashSet<String>>,
    closing: Mutex<HashSet<String>>,
}

impl Archives {
    /// `dir` holds downloads and unpacked tars; leftovers from an earlier
    /// run are removed.
    pub fn new(registry: Arc<Registry>, queue: OperationQueue, dir: PathBuf) -> Self {
        let _ = std::fs::remove_dir_all(&dir);
        Self {
            registry,
            queue,
            dir,
            open: Mutex::new(HashSet::new()),
            closing: Mutex::new(HashSet::new()),
        }
    }

    /// Open the archive at `location` and register it. Returns its file
    /// system id and name.
    pub async fn open(
        &self,
        location: &Location,
        cancel: &CancellationToken,
        progress: ProgressFn,
    ) -> Result<Arc<ArchiveFs>> {
        let id = format!("{ID_PREFIX}{}", uuid::Uuid::new_v4().simple());
        let fs = Arc::new(
            open_archive(
                &self.registry,
                location,
                id.clone(),
                &self.dir,
                cancel,
                progress,
            )
            .await?,
        );
        self.registry.register(fs.clone());
        self.open.lock().unwrap().insert(id);
        Ok(fs)
    }

    /// Ids of the archives that are open.
    pub fn ids(&self) -> Vec<String> {
        let mut v: Vec<_> = self.open.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    /// Whether an operation uses the archive `id`: one queued or running,
    /// or one that stopped on an existing file, which can be retried with
    /// "Replace existing" or "Skip existing" until it is cleared.
    fn busy(&self, id: &str) -> bool {
        self.queue.list().iter().any(|o| {
            let retryable = o.status == OperationStatus::Failed
                && o.error.as_ref().is_some_and(|e| e.kind == "alreadyExists");
            (retryable || matches!(o.status, OperationStatus::Queued | OperationStatus::Running))
                && o.places.iter().any(|p| p == id)
        })
    }

    /// Close the archive `id` now, or once the operations using it end.
    pub fn close(&self, id: &str) {
        if !self.open.lock().unwrap().contains(id) {
            return;
        }
        if self.busy(id) {
            self.closing.lock().unwrap().insert(id.to_string());
        } else {
            self.unregister(id);
        }
    }

    /// Close the archives waiting for their operations to end (or to be
    /// cleared).
    pub fn reap(&self) {
        let ids: Vec<String> = self.closing.lock().unwrap().iter().cloned().collect();
        for id in ids {
            if !self.busy(&id) {
                self.unregister(&id);
            }
        }
    }

    fn unregister(&self, id: &str) {
        self.closing.lock().unwrap().remove(id);
        self.open.lock().unwrap().remove(id);
        // The scratch folder goes when the last reader drops the archive.
        self.registry.unregister(id);
    }

    /// Close every archive and remove the scratch folders (app exit).
    pub fn close_all(&self) {
        for id in self.ids() {
            self.unregister(&id);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// VFS-ARC-005
    fn names_never_leave_the_archive() {
        let p = |s: &str| clean_parts(s, true).join("/");
        assert_eq!(p("../evil.txt"), "evil.txt");
        assert_eq!(p("/etc/passwd"), "etc/passwd");
        assert_eq!(p("a/../../b"), "b");
        assert_eq!(p("a/./b//c"), "a/b/c");
        assert_eq!(p("C:\\Windows\\x.dll"), "Windows/x.dll");
        assert_eq!(p(".."), "");
        assert_eq!(clean_parts("a\\b", false).len(), 1);
        for part in clean_parts("x/\0y/..\\..\\z", true) {
            assert!(is_safe_local_name(&part), "{part}");
        }
    }

    /// Open the 7z `file`, then treat more than 1 MiB as big.
    fn open_small_limit(file: &Path) -> ArchiveFs {
        let scratch = Scratch::create(file.with_extension("scratch")).unwrap();
        let progress: ProgressFn = Arc::new(|_| {});
        let mut fs = ArchiveFs::open_blocking(
            "archive:t".into(),
            file.to_path_buf(),
            "bomb.7z".into(),
            Location::new(LocalFs::ID, file.to_string_lossy()),
            scratch,
            &CancellationToken::new(),
            &progress,
        )
        .unwrap();
        Arc::get_mut(&mut fs.inner).unwrap().bomb_min = 1 << 20;
        fs
    }

    const ZEROS: usize = 8 << 20;

    #[test]
    /// VFS-ARC-007: a compressed tar that unpacks to more than 1000 times its
    /// size is refused while it unpacks.
    fn compressed_tar_bombs_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(ZEROS as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "zeros.bin", io::repeat(0).take(ZEROS as u64))
            .unwrap();
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
        e.write_all(&b.into_inner().unwrap()).unwrap();
        let path = tmp.path().join("bomb.tar.bz2");
        std::fs::write(&path, e.finish().unwrap()).unwrap();
        let e = unpack_tar(
            Format::TarBz2,
            &path,
            &tmp.path().join("unpacked.tar"),
            "bomb.tar.bz2",
            &CancellationToken::new(),
            &|_, _| {},
            1 << 20,
        )
        .unwrap_err();
        assert!(e.to_string().contains("zip bomb"), "{e}");
    }

    #[tokio::test]
    /// VFS-ARC-007: a 7z file whose block unpacks to more than 1000 times
    /// its size is refused before it is decoded.
    async fn seven_z_bombs_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bomb.7z");
        let mut w = sevenz_rust2::ArchiveWriter::new(File::create(&path).unwrap()).unwrap();
        w.push_archive_entry(
            sevenz_rust2::ArchiveEntry::new_file("zeros.bin"),
            Some(io::repeat(0).take(ZEROS as u64)),
        )
        .unwrap();
        w.finish().unwrap();
        let fs = open_small_limit(&path);
        let e = fs.open_read("/zeros.bin").await.err().expect("read");
        assert!(e.to_string().contains("zip bomb"), "{e}");
    }

    #[test]
    /// VFS-ARC-001
    fn recognises_archive_names() {
        for n in [
            "a.zip",
            "A.ZIP",
            "a.tar",
            "a.tar.gz",
            "a.tgz",
            "a.tar.bz2",
            "a.tbz2",
            "a.tar.xz",
            "a.txz",
            "a.tar.zst",
            "a.tzst",
            "a.7z",
            "a.gz",
            "a.log.bz2",
            "a.xz",
            "a.ZST",
            "a.rar",
            "A.RAR",
        ] {
            assert!(is_archive_name(n), "{n}");
        }
        assert_eq!(format_from_name("a.tar.zst"), Some(Format::TarZst));
        assert_eq!(format_from_name("a.txt.gz"), Some(Format::Gz));
        assert_eq!(format_from_name("a.tar.gz"), Some(Format::TarGz));
        assert_eq!(format_from_name("a.rar"), Some(Format::Rar));
        for n in ["zip", ".zip", ".gz", "a.txt", "a.gzip"] {
            assert!(!is_archive_name(n), "{n}");
        }
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("x.ZIP"), "x");
        assert_eq!(archive_stem("plain"), "plain");
        assert_eq!(archive_stem("notes.txt.gz"), "notes.txt");
        assert_eq!(archive_stem("site.tar.zst"), "site");
    }
}

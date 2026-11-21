//! File checksums, for the Checksums dialog.
//!
//! [`calculate`] hashes files and folders with MD5, SHA-1, SHA-256 or XXH128.
//! [`verify`] checks the files listed in a checksum file. Both read files
//! through the [`FileSystem`] trait in fixed-size chunks, so they work the same
//! on the local disk, a server or a bucket, and never hold a whole file in
//! memory. Checksum files use the GNU coreutils layout (`<hex>  <name>`, or
//! `<hex> *<name>` in binary mode), with paths relative to the checksum file's
//! folder. [`parse_lines`] and [`format_lines`] read and write that layout.

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
use xxhash_rust::xxh3::Xxh3;

use crate::error::{Error, Result};
use crate::vfs::{EntryKind, FileSystem, Location, Registry};

/// Bytes read from a file per step. Progress is reported after each step.
const CHUNK: usize = 256 * 1024;

/// A hash algorithm the Checksums dialog offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    Md5,
    Sha1,
    Sha256,
    /// XXH3 with a 128-bit output, as printed by `xxh128sum`.
    Xxh128,
}

impl Algorithm {
    /// Every algorithm, in the order the dialog lists them.
    pub const ALL: [Algorithm; 4] = [
        Algorithm::Md5,
        Algorithm::Sha1,
        Algorithm::Sha256,
        Algorithm::Xxh128,
    ];

    /// Extension of checksum files for this algorithm, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Algorithm::Md5 => "md5",
            Algorithm::Sha1 => "sha1",
            Algorithm::Sha256 => "sha256",
            Algorithm::Xxh128 => "xxh128",
        }
    }

    /// Length of the hex digest in characters.
    pub fn hex_len(self) -> usize {
        match self {
            Algorithm::Md5 | Algorithm::Xxh128 => 32,
            Algorithm::Sha1 => 40,
            Algorithm::Sha256 => 64,
        }
    }

    /// The algorithm named by a checksum file's extension, e.g. `"SUMS.md5"`.
    pub fn from_file_name(name: &str) -> Option<Algorithm> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Algorithm::ALL.into_iter().find(|a| a.extension() == ext)
    }

    fn hasher(self) -> Hasher {
        match self {
            Algorithm::Md5 => Hasher::Md5(Md5::new()),
            Algorithm::Sha1 => Hasher::Sha1(Sha1::new()),
            Algorithm::Sha256 => Hasher::Sha256(Sha256::new()),
            Algorithm::Xxh128 => Hasher::Xxh128(Box::new(Xxh3::new())),
        }
    }
}

/// Running state of one digest.
enum Hasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    // Boxed: the XXH3 state is much larger than the other variants.
    Xxh128(Box<Xxh3>),
}

impl Hasher {
    fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Md5(h) => h.update(data),
            Hasher::Sha1(h) => h.update(data),
            Hasher::Sha256(h) => h.update(data),
            Hasher::Xxh128(h) => h.update(data),
        }
    }

    /// The digest as lowercase hex.
    fn finish_hex(self) -> String {
        match self {
            Hasher::Md5(h) => to_hex(&h.finalize()),
            Hasher::Sha1(h) => to_hex(&h.finalize()),
            Hasher::Sha256(h) => to_hex(&h.finalize()),
            // The canonical form of xxh128sum: the 128-bit value, big-endian.
            Hasher::Xxh128(h) => format!("{:032x}", h.digest128()),
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash the bytes of `data` in one go. Used for short strings and tests.
pub fn hash_bytes(algorithm: Algorithm, data: &[u8]) -> String {
    let mut h = algorithm.hasher();
    h.update(data);
    h.finish_hex()
}

/// Hash the file at `path` on `fs`, reading it in chunks. `on_bytes` gets the
/// size of each chunk as it is read. Stops with [`Error::Cancelled`] once
/// `cancel` is set.
pub async fn hash_file(
    fs: &dyn FileSystem,
    path: &str,
    algorithm: Algorithm,
    cancel: &CancellationToken,
    mut on_bytes: impl FnMut(u64),
) -> Result<String> {
    let mut reader = tokio::select! {
        r = fs.open_read(path) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    let mut hasher = algorithm.hasher();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = tokio::select! {
            r = reader.read(&mut buf) => r.map_err(|e| Error::io(path, e))?,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        on_bytes(n as u64);
    }
    Ok(hasher.finish_hex())
}

/// Progress of a running calculation or verification.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksumProgress {
    /// Files finished so far.
    pub files_done: u64,
    /// Files to hash in all. For a verification this is the number of lines
    /// in the checksum file.
    pub files_total: u64,
    /// Bytes read so far.
    pub bytes_done: u64,
    /// Bytes to read in all, when known (zero for a verification).
    pub bytes_total: u64,
    /// Path of the file being hashed, relative to the folder.
    pub current: String,
}

/// One file in a calculation. Exactly one of `hash` and `error` is set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksumEntry {
    /// Path relative to the folder, with `/` between the parts.
    pub path: String,
    /// Lowercase hex digest.
    pub hash: Option<String>,
    /// Why the file could not be hashed, e.g. a folder that could not be read.
    pub error: Option<String>,
}

/// Hash `names` (items directly inside `folder`) with `algorithm`. Folders
/// are walked recursively; symbolic links to folders below the top level are
/// not followed. Results are sorted by path. A file that cannot be read gets
/// an entry with `error` set and the walk goes on. `on_progress` is called
/// as bytes are read. Stops with [`Error::Cancelled`] once `cancel` is set.
pub async fn calculate(
    registry: &Registry,
    folder: &Location,
    names: &[String],
    algorithm: Algorithm,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(ChecksumProgress),
) -> Result<Vec<ChecksumEntry>> {
    let fs = registry.get(&folder.fs)?;
    let (files, mut entries) = collect(fs.as_ref(), &folder.path, names, cancel).await?;

    let mut progress = ChecksumProgress {
        files_total: files.len() as u64,
        bytes_total: files.iter().map(|f| f.size).sum(),
        ..Default::default()
    };
    for file in &files {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        progress.current = file.rel.clone();
        on_progress(progress.clone());
        let hashed = hash_file(fs.as_ref(), &file.path, algorithm, cancel, |n| {
            progress.bytes_done += n;
            on_progress(progress.clone());
        })
        .await;
        match hashed {
            Ok(hash) => entries.push(ChecksumEntry {
                path: file.rel.clone(),
                hash: Some(hash),
                error: None,
            }),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => entries.push(ChecksumEntry {
                path: file.rel.clone(),
                hash: None,
                error: Some(e.to_string()),
            }),
        }
        progress.files_done += 1;
        on_progress(progress.clone());
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// A regular file to hash.
struct Planned {
    /// Path relative to the folder, with `/` between the parts.
    rel: String,
    path: String,
    size: u64,
}

/// Find the files below `names` (directly inside `folder_path`). Items that
/// cannot be read and folders that cannot be listed come back as error
/// entries rather than failing the walk.
async fn collect(
    fs: &dyn FileSystem,
    folder_path: &str,
    names: &[String],
    cancel: &CancellationToken,
) -> Result<(Vec<Planned>, Vec<ChecksumEntry>)> {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    // Folders still to list, with the relative path of each.
    let mut pending: Vec<(String, String)> = Vec::new();
    for name in names {
        let path = fs.join(folder_path, name);
        let stat = tokio::select! {
            r = fs.stat(&path) => r,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let entry = match stat {
            Ok(e) => e,
            Err(e) => {
                errors.push(ChecksumEntry {
                    path: name.clone(),
                    hash: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        match entry.kind {
            EntryKind::Dir => pending.push((path, name.clone())),
            EntryKind::File => files.push(Planned {
                rel: name.clone(),
                path,
                size: entry.size,
            }),
            EntryKind::Other => {}
        }
    }
    while let Some((dir, rel)) = pending.pop() {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let listing = tokio::select! {
            r = fs.list(&dir) => r,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let listing = match listing {
            Ok(l) => l,
            Err(e) => {
                errors.push(ChecksumEntry {
                    path: rel,
                    hash: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        for e in listing {
            let child_rel = format!("{rel}/{}", e.name);
            match e.kind {
                // A symbolic link to a folder could loop back up the tree.
                EntryKind::Dir if !e.symlink => pending.push((e.path, child_rel)),
                EntryKind::Dir => {}
                EntryKind::File => files.push(Planned {
                    rel: child_rel,
                    path: e.path,
                    size: e.size,
                }),
                EntryKind::Other => {}
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((files, errors))
}

/// Escape a path for a checksum line. Backslashes, newlines and carriage
/// returns are written as `\\`, `\n` and `\r`, and such a line starts with a
/// backslash (GNU style).
fn escape_path(path: &str) -> (bool, String) {
    if path.contains(['\\', '\n', '\r']) {
        let escaped = path
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        (true, escaped)
    } else {
        (false, path.to_string())
    }
}

fn unescape_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Write checksum lines in the GNU coreutils layout, in text mode (two spaces
/// between digest and name). Entries with an error are left out.
pub fn format_lines(entries: &[ChecksumEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let Some(hash) = &e.hash else { continue };
        let (escaped, path) = escape_path(&e.path);
        if escaped {
            out.push('\\');
        }
        out.push_str(hash);
        out.push_str("  ");
        out.push_str(&path);
        out.push('\n');
    }
    out
}

/// One line of a checksum file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumLine {
    /// Lowercase hex digest.
    pub hash: String,
    /// Path as written in the file, relative to the checksum file's folder.
    pub path: String,
}

/// Read one line of a checksum file. Blank lines give `Ok(None)`. `line_no`
/// is only used in the error message.
pub fn parse_line(
    line: &str,
    algorithm: Algorithm,
    line_no: usize,
) -> Result<Option<ChecksumLine>> {
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() {
        return Ok(None);
    }
    let bad = || {
        Error::Invalid(format!(
            "line {line_no} is not a {} checksum",
            algorithm.extension()
        ))
    };
    // A leading backslash marks an escaped name.
    let (escaped, body) = match line.strip_prefix('\\') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let len = algorithm.hex_len();
    if body.len() < len + 2 || !body.is_char_boundary(len) {
        return Err(bad());
    }
    let (hash, rest) = body.split_at(len);
    if !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    // Text mode is "  ", binary mode is " *".
    let name = match rest.get(..2) {
        Some("  ") | Some(" *") => &rest[2..],
        _ => return Err(bad()),
    };
    if name.is_empty() {
        return Err(bad());
    }
    let path = if escaped {
        unescape_path(name)
    } else {
        name.to_string()
    };
    Ok(Some(ChecksumLine {
        hash: hash.to_ascii_lowercase(),
        path,
    }))
}

/// Read all the lines of a checksum file's text. A line that is not a
/// checksum for `algorithm` fails with [`Error::Invalid`].
pub fn parse_lines(text: &str, algorithm: Algorithm) -> Result<Vec<ChecksumLine>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(l) = parse_line(line, algorithm, i + 1)? {
            out.push(l);
        }
    }
    Ok(out)
}

/// Write the checksum file `file_name` in `folder`, holding the hashes in
/// `entries`. The name always ends with the extension of `algorithm`: a
/// checksum extension already on the name is replaced. With `overwrite` off,
/// an existing file gives [`Error::AlreadyExists`]. Returns the new file.
pub async fn write_file(
    registry: &Registry,
    folder: &Location,
    file_name: &str,
    algorithm: Algorithm,
    entries: &[ChecksumEntry],
    overwrite: bool,
) -> Result<Location> {
    let fs = registry.get(&folder.fs)?;
    let name = checksum_name(file_name, algorithm);
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(Error::Invalid(format!("not a file name: {file_name}")));
    }
    let path = fs.join(&folder.path, &name);
    if !overwrite && fs.exists(&path).await? {
        return Err(Error::AlreadyExists(path));
    }
    let text = format_lines(entries);
    let mut writer = fs.open_write(&path).await?;
    writer
        .write_all(text.as_bytes())
        .await
        .map_err(|e| Error::io(&path, e))?;
    writer.shutdown().await.map_err(|e| Error::io(&path, e))?;
    Ok(Location::new(folder.fs.clone(), path))
}

/// `file_name` with its checksum extension (of any algorithm) replaced by
/// the one for `algorithm`, which is added when there is none.
fn checksum_name(file_name: &str, algorithm: Algorithm) -> String {
    let trimmed = file_name.trim();
    let stem = match trimmed.rsplit_once('.') {
        Some((stem, _)) if Algorithm::from_file_name(trimmed).is_some() => stem,
        _ => trimmed,
    };
    if stem.is_empty() {
        return String::new();
    }
    format!("{stem}.{}", algorithm.extension())
}

/// The verdict for one line of a checksum file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyStatus {
    /// The file's digest matches.
    Ok,
    /// The digest differs, or the file could not be read.
    Failed,
    /// The file is not there (or is a folder).
    Missing,
}

/// One checked file in a verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyEntry {
    pub path: String,
    pub status: VerifyStatus,
    /// The digest in the checksum file.
    pub expected: String,
    /// The digest of the file, when it could be read.
    pub actual: Option<String>,
    /// Why the file could not be read, when it could not.
    pub error: Option<String>,
}

/// The outcome of [`verify`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    /// One entry per line of the checksum file, in file order.
    pub entries: Vec<VerifyEntry>,
    pub ok: u64,
    pub failed: u64,
    pub missing: u64,
}

/// Check every file listed in the checksum file `checksum` against its
/// digest. Paths are relative to the checksum file's folder. The file is
/// read line by line; a malformed line fails the whole check with
/// [`Error::Invalid`]. Stops with [`Error::Cancelled`] once `cancel` is set.
pub async fn verify(
    registry: &Registry,
    checksum: &Location,
    algorithm: Algorithm,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(ChecksumProgress),
) -> Result<VerifyReport> {
    let fs = registry.get(&checksum.fs)?;
    let lines = read_lines(fs.as_ref(), &checksum.path, algorithm, cancel).await?;
    let base = fs
        .parent(&checksum.path)
        .ok_or_else(|| Error::Invalid(format!("no folder for {}", checksum.path)))?;

    let mut progress = ChecksumProgress {
        files_total: lines.len() as u64,
        ..Default::default()
    };
    let mut report = VerifyReport::default();
    for line in lines {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        progress.current = line.path.clone();
        on_progress(progress.clone());
        let path = line
            .path
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .fold(base.clone(), |dir, part| fs.join(&dir, part));
        let entry = check_one(fs.as_ref(), &path, &line, algorithm, cancel).await?;
        match entry.status {
            VerifyStatus::Ok => report.ok += 1,
            VerifyStatus::Failed => report.failed += 1,
            VerifyStatus::Missing => report.missing += 1,
        }
        report.entries.push(entry);
        progress.files_done += 1;
        on_progress(progress.clone());
    }
    Ok(report)
}

/// Read every checksum line of the file at `path`, streaming it.
async fn read_lines(
    fs: &dyn FileSystem,
    path: &str,
    algorithm: Algorithm,
    cancel: &CancellationToken,
) -> Result<Vec<ChecksumLine>> {
    let reader = tokio::select! {
        r = fs.open_read(path) => r?,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    let mut lines = BufReader::new(reader).lines();
    let mut out = Vec::new();
    let mut n = 0usize;
    loop {
        let next = tokio::select! {
            r = lines.next_line() => r.map_err(|e| Error::io(path, e))?,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let Some(text) = next else { break };
        n += 1;
        if let Some(line) = parse_line(&text, algorithm, n)? {
            out.push(line);
        }
    }
    Ok(out)
}

async fn check_one(
    fs: &dyn FileSystem,
    path: &str,
    line: &ChecksumLine,
    algorithm: Algorithm,
    cancel: &CancellationToken,
) -> Result<VerifyEntry> {
    let expected = line.hash.clone();
    let entry = |status, actual, error| VerifyEntry {
        path: line.path.clone(),
        status,
        expected: expected.clone(),
        actual,
        error,
    };
    let stat = tokio::select! {
        r = fs.stat(path) => r,
        _ = cancel.cancelled() => return Err(Error::Cancelled),
    };
    match stat {
        Err(Error::NotFound(_)) => return Ok(entry(VerifyStatus::Missing, None, None)),
        Err(e) => return Ok(entry(VerifyStatus::Failed, None, Some(e.to_string()))),
        Ok(e) if e.kind != EntryKind::File => {
            return Ok(entry(VerifyStatus::Missing, None, None));
        }
        Ok(_) => {}
    }
    match hash_file(fs, path, algorithm, cancel, |_| {}).await {
        Ok(actual) => {
            let status = if actual == expected {
                VerifyStatus::Ok
            } else {
                VerifyStatus::Failed
            };
            Ok(entry(status, Some(actual), None))
        }
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(e) => Ok(entry(VerifyStatus::Failed, None, Some(e.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known vectors: RFC 1321 / FIPS 180 test strings, and the empty input.
    #[test]
    fn known_vectors() {
        let cases = [
            (Algorithm::Md5, "", "d41d8cd98f00b204e9800998ecf8427e"),
            (Algorithm::Md5, "abc", "900150983cd24fb0d6963f7d28e17f72"),
            (
                Algorithm::Sha1,
                "",
                "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            ),
            (
                Algorithm::Sha1,
                "abc",
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                Algorithm::Sha256,
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                Algorithm::Sha256,
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (Algorithm::Xxh128, "", "99aa06d3014798d86001c324468d497f"),
            (Algorithm::Xxh128, "abc", "06b05ab6733a618578af5f94892f3950"),
        ];
        for (algo, input, expected) in cases {
            assert_eq!(
                hash_bytes(algo, input.as_bytes()),
                expected,
                "{algo:?} of {input:?}"
            );
        }
    }

    #[test]
    fn streaming_matches_one_shot() {
        // Feed the hasher in odd-sized pieces, as the chunked reader does.
        let data: Vec<u8> = (0..1_000_003u32).map(|i| (i % 251) as u8).collect();
        for algo in Algorithm::ALL {
            let mut h = algo.hasher();
            for piece in data.chunks(777) {
                h.update(piece);
            }
            assert_eq!(h.finish_hex(), hash_bytes(algo, &data), "{algo:?}");
        }
    }

    #[test]
    fn algorithm_from_extension() {
        assert_eq!(Algorithm::from_file_name("a.MD5"), Some(Algorithm::Md5));
        assert_eq!(
            Algorithm::from_file_name("SUMS.sha256"),
            Some(Algorithm::Sha256)
        );
        assert_eq!(
            Algorithm::from_file_name("x.xxh128"),
            Some(Algorithm::Xxh128)
        );
        assert_eq!(Algorithm::from_file_name("photo.jpg"), None);
        assert_eq!(Algorithm::from_file_name("md5"), None);
    }

    #[test]
    fn format_and_parse_round_trip() {
        let entries = vec![
            ChecksumEntry {
                path: "a.txt".into(),
                hash: Some("900150983cd24fb0d6963f7d28e17f72".into()),
                error: None,
            },
            ChecksumEntry {
                path: "sub/b c.txt".into(),
                hash: Some("d41d8cd98f00b204e9800998ecf8427e".into()),
                error: None,
            },
            ChecksumEntry {
                path: "odd\\name\nx".into(),
                hash: Some("900150983cd24fb0d6963f7d28e17f72".into()),
                error: None,
            },
            ChecksumEntry {
                path: "broken".into(),
                hash: None,
                error: Some("denied".into()),
            },
        ];
        let text = format_lines(&entries);
        assert_eq!(
            text,
            "900150983cd24fb0d6963f7d28e17f72  a.txt\n\
             d41d8cd98f00b204e9800998ecf8427e  sub/b c.txt\n\
             \\900150983cd24fb0d6963f7d28e17f72  odd\\\\name\\nx\n"
        );
        let parsed = parse_lines(&text, Algorithm::Md5).unwrap();
        let expected: Vec<(String, String)> = entries
            .iter()
            .filter_map(|e| e.hash.clone().map(|h| (h, e.path.clone())))
            .collect();
        let got: Vec<(String, String)> = parsed.into_iter().map(|l| (l.hash, l.path)).collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn carriage_returns_in_names_round_trip() {
        let entries = vec![ChecksumEntry {
            path: "a\r".into(),
            hash: Some("900150983cd24fb0d6963f7d28e17f72".into()),
            error: None,
        }];
        let text = format_lines(&entries);
        assert_eq!(text, "\\900150983cd24fb0d6963f7d28e17f72  a\\r\n");
        let parsed = parse_lines(&text, Algorithm::Md5).unwrap();
        assert_eq!(parsed[0].path, "a\r");
    }

    #[test]
    fn parses_binary_mode_and_crlf_and_uppercase() {
        let line = parse_line(
            "900150983CD24FB0D6963F7D28E17F72 *a.bin\r",
            Algorithm::Md5,
            1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(line.hash, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(line.path, "a.bin");
        assert_eq!(parse_line("   ", Algorithm::Md5, 2).unwrap(), None);
    }

    #[test]
    fn rejects_malformed_lines() {
        // Wrong length for SHA-1, not hex, and no separator.
        for bad in [
            "900150983cd24fb0d6963f7d28e17f72  a.txt",
            "zz0150983cd24fb0d6963f7d28e17f72  a.txt",
            "900150983cd24fb0d6963f7d28e17f72a.txt",
            "900150983cd24fb0d6963f7d28e17f72  ",
        ] {
            let err = parse_line(bad, Algorithm::Sha1, 3).unwrap_err();
            assert_eq!(err.kind(), "invalid", "{bad}");
        }
        let err =
            parse_lines("\n900150983cd24fb0d6963f7d28e17f72 x\n", Algorithm::Md5).unwrap_err();
        assert!(err.to_string().contains("line 2"), "{err}");
    }

    #[test]
    fn checksum_names() {
        assert_eq!(checksum_name("photo.jpg", Algorithm::Md5), "photo.jpg.md5");
        assert_eq!(checksum_name("SUMS.SHA1", Algorithm::Sha256), "SUMS.sha256");
        assert_eq!(checksum_name("SUMS", Algorithm::Xxh128), "SUMS.xxh128");
    }
}

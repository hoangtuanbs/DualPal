//! The file viewer's core (F3): text encodings, streamed search and line
//! counting over any [`FileSystem`], and the open files the UI reads pages of.
//!
//! Nothing here loads a whole file. The UI reads pages through a
//! [`RangeReader`] held in [`Sessions`]; search and line counts stream the
//! file in chunks and can be cancelled. The file compare view reuses the
//! same pieces for its two sides.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{FileSystem, RangeReader};

// ---- encodings -----------------------------------------------------------------

/// A text encoding the viewer can show. Serialized with the WHATWG labels
/// (`TextDecoder` names) the UI uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Encoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "utf-16le")]
    Utf16Le,
    #[serde(rename = "utf-16be")]
    Utf16Be,
    #[serde(rename = "windows-1252")]
    Windows1252,
    /// ISO-8859-1: every byte is the code point of the same number.
    #[serde(rename = "iso-8859-1")]
    Latin1,
}

impl Encoding {
    /// Bytes per code unit: 2 for UTF-16, else 1.
    pub fn unit(self) -> usize {
        match self {
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
            _ => 1,
        }
    }

    /// The line feed in this encoding.
    pub fn newline(self) -> &'static [u8] {
        match self {
            Encoding::Utf16Le => b"\n\0",
            Encoding::Utf16Be => b"\0\n",
            _ => b"\n",
        }
    }
}

/// What the start of a file says about its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub encoding: Encoding,
    /// Length of the byte order mark the text starts after (0 without one).
    pub bom: u32,
    /// The bytes do not look like text (NUL bytes outside UTF-16).
    pub binary: bool,
}

/// Guess the encoding of a file from its first bytes. `complete` says the
/// sample is the whole file, so a multi-byte character cut at its end is an
/// error rather than the read limit.
///
/// A byte order mark wins. Without one, UTF-16 is recognised by its zero
/// high bytes, valid UTF-8 is UTF-8, and anything else is taken as
/// Windows-1252 (a superset of Latin-1 for printable text).
pub fn detect_encoding(sample: &[u8], complete: bool) -> Detected {
    let with = |encoding, bom, binary| Detected {
        encoding,
        bom,
        binary,
    };
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return with(Encoding::Utf8, 3, false);
    }
    if sample.starts_with(&[0xFF, 0xFE]) {
        return with(Encoding::Utf16Le, 2, false);
    }
    if sample.starts_with(&[0xFE, 0xFF]) {
        return with(Encoding::Utf16Be, 2, false);
    }
    if let Some(enc) = utf16_without_bom(sample) {
        return with(enc, 0, false);
    }
    let binary = sample.contains(&0);
    match std::str::from_utf8(sample) {
        Ok(_) => with(Encoding::Utf8, 0, binary),
        // Only the last character is cut off by the end of the sample.
        Err(e) if !complete && e.error_len().is_none() => with(Encoding::Utf8, 0, binary),
        Err(_) => with(Encoding::Windows1252, 0, binary),
    }
}

/// UTF-16 text without a BOM has mostly zero bytes on one side: ASCII
/// characters are `xx 00` (LE) or `00 xx` (BE).
fn utf16_without_bom(sample: &[u8]) -> Option<Encoding> {
    let pairs = sample.len() / 2;
    if pairs < 4 {
        return None;
    }
    let (mut even_zero, mut odd_zero) = (0usize, 0usize);
    for pair in sample.chunks_exact(2) {
        even_zero += (pair[0] == 0) as usize;
        odd_zero += (pair[1] == 0) as usize;
    }
    let mostly = |n: usize| n * 10 >= pairs * 7;
    let rarely = |n: usize| n * 10 <= pairs;
    if mostly(odd_zero) && rarely(even_zero) {
        Some(Encoding::Utf16Le)
    } else if mostly(even_zero) && rarely(odd_zero) {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// Windows-1252 characters for bytes 0x80..=0x9F (the rest equal Latin-1).
/// Undefined bytes map to the C1 control of the same number, as browsers do.
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decode `bytes` as `encoding`, replacing what cannot be decoded with
/// U+FFFD. A trailing odd byte of UTF-16 is dropped.
pub fn decode(bytes: &[u8], encoding: Encoding) -> String {
    match encoding {
        Encoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let units = bytes.chunks_exact(2).map(|p| {
                if encoding == Encoding::Utf16Le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            });
            char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect()
        }
        Encoding::Latin1 => bytes.iter().map(|&b| b as char).collect(),
        Encoding::Windows1252 => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => {
                    char::from_u32(CP1252_HIGH[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
                }
                _ => b as char,
            })
            .collect(),
    }
}

/// Encode `text` as `encoding`, e.g. a search term. `None` when a character
/// has no byte in a single-byte encoding.
pub fn encode(text: &str, encoding: Encoding) -> Option<Vec<u8>> {
    match encoding {
        Encoding::Utf8 => Some(text.as_bytes().to_vec()),
        Encoding::Utf16Le => Some(text.encode_utf16().flat_map(u16::to_le_bytes).collect()),
        Encoding::Utf16Be => Some(text.encode_utf16().flat_map(u16::to_be_bytes).collect()),
        Encoding::Latin1 => text.chars().map(|c| u8::try_from(c as u32).ok()).collect(),
        Encoding::Windows1252 => text
            .chars()
            .map(|c| {
                let cp = c as u32;
                if let Some(i) = CP1252_HIGH.iter().position(|&h| h as u32 == cp) {
                    return Some(0x80 + i as u8);
                }
                match cp {
                    0..=0x7F | 0xA0..=0xFF => Some(cp as u8),
                    _ => None,
                }
            })
            .collect(),
    }
}

// ---- streamed search -------------------------------------------------------------

/// Bytes read per step by [`search`] and [`count_lines`].
pub const SEARCH_CHUNK: usize = 1 << 20;

#[derive(Clone, Copy, Debug)]
pub struct SearchOptions {
    /// Find the last match that starts before `from` instead of the first
    /// one at or after it.
    pub backward: bool,
    /// Compare ASCII letters without case.
    pub ignore_case: bool,
    /// Bytes read per step (tests use small ones to cross boundaries).
    pub chunk: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            backward: false,
            ignore_case: false,
            chunk: SEARCH_CHUNK,
        }
    }
}

fn fold(bytes: &mut [u8], ignore_case: bool) {
    if ignore_case {
        bytes.make_ascii_lowercase();
    }
}

/// Find `needle` in the file `path` of `size` bytes, starting at `from`.
/// Returns the offset of the match. The file is streamed in chunks, so a
/// match can be anywhere, including across chunk boundaries; `progress` is
/// told how many bytes have been looked at. `cancel` stops the search with
/// [`Error::Cancelled`].
#[allow(clippy::too_many_arguments)]
pub async fn search(
    fs: &dyn FileSystem,
    path: &str,
    size: u64,
    needle: &[u8],
    from: u64,
    options: SearchOptions,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64),
) -> Result<Option<u64>> {
    if needle.is_empty() || needle.len() as u64 > size {
        return Ok(None);
    }
    let mut needle = needle.to_vec();
    fold(&mut needle, options.ignore_case);
    let chunk = options.chunk.max(needle.len());
    let from = from.min(size);

    if !options.backward {
        return scan_forward(
            fs,
            path,
            (from, size),
            &needle,
            chunk,
            options.ignore_case,
            false,
            cancel,
            &mut progress,
        )
        .await;
    }
    if !fs.seekable_reads() {
        // Reading backwards would stream the start again for every chunk:
        // read forwards once and keep the last match before `from`.
        let end = (from + needle.len() as u64 - 1).min(size);
        return scan_forward(
            fs,
            path,
            (0, end),
            &needle,
            chunk,
            options.ignore_case,
            true,
            cancel,
            &mut progress,
        )
        .await;
    }
    // Chunks from `from` towards the start. Each chunk reaches
    // `needle.len() - 1` bytes into the one after it, so a match across the
    // boundary is found.
    let n = needle.len() as u64;
    let mut end = (from + n - 1).min(size);
    let mut scanned = 0u64;
    while end >= n {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let start = end.saturating_sub(chunk as u64);
        let reader = fs.open_read_at(path, start).await?;
        let mut buf = crate::vfs::range::read_up_to(reader, end - start, path).await?;
        fold(&mut buf, options.ignore_case);
        if let Some(i) = memchr::memmem::rfind(&buf, &needle) {
            let at = start + i as u64;
            if at < from {
                return Ok(Some(at));
            }
        }
        scanned += end - start;
        progress(scanned);
        if start == 0 {
            break;
        }
        end = start + n - 1;
    }
    Ok(None)
}

/// Stream `range` forwards and find the first match in it, or with `last`
/// the last match that starts before `range.1 - needle_len + 1`.
#[allow(clippy::too_many_arguments)]
async fn scan_forward(
    fs: &dyn FileSystem,
    path: &str,
    range: (u64, u64),
    needle: &[u8],
    chunk: usize,
    ignore_case: bool,
    last: bool,
    cancel: &CancellationToken,
    progress: &mut impl FnMut(u64),
) -> Result<Option<u64>> {
    let (from, end) = range;
    let needle_len = needle.len();
    if end <= from || end - from < needle_len as u64 {
        return Ok(None);
    }
    let mut reader = fs.open_read_at(path, from).await?;
    // `buf` holds the bytes from `buf_start`: the tail of the last chunk
    // (needle_len - 1 bytes) and then the new one.
    let mut buf: Vec<u8> = Vec::with_capacity(chunk + needle_len);
    let mut buf_start = from;
    let mut pos = from;
    let mut found = None;
    while pos < end {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let want = (end - pos).min(chunk as u64);
        let before = buf.len();
        (&mut reader)
            .take(want)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| Error::io(path, e))?;
        let got = buf.len() - before;
        if got == 0 {
            break; // The file is shorter than it was.
        }
        pos += got as u64;
        fold(&mut buf[before..], ignore_case);
        if last {
            if let Some(i) = memchr::memmem::rfind(&buf, needle) {
                found = Some(buf_start + i as u64);
            }
        } else if let Some(i) = memchr::memmem::find(&buf, needle) {
            return Ok(Some(buf_start + i as u64));
        }
        progress(pos - from);
        let keep = (needle_len - 1).min(buf.len());
        buf.drain(..buf.len() - keep);
        buf_start = pos - keep as u64;
    }
    Ok(found)
}

/// How many line feeds are in the first `upto` bytes of the file: the
/// zero-based number of the line that `upto` is on, when `upto` is a line
/// start. Streams the file; `cancel` stops it.
pub async fn count_lines(
    fs: &dyn FileSystem,
    path: &str,
    upto: u64,
    encoding: Encoding,
    cancel: &CancellationToken,
) -> Result<u64> {
    count_lines_chunked(fs, path, upto, encoding, cancel, SEARCH_CHUNK).await
}

/// [`count_lines`] reading `chunk` bytes per step.
pub async fn count_lines_chunked(
    fs: &dyn FileSystem,
    path: &str,
    upto: u64,
    encoding: Encoding,
    cancel: &CancellationToken,
    chunk: usize,
) -> Result<u64> {
    if upto == 0 {
        return Ok(0);
    }
    let mut reader = fs.open_read_at(path, 0).await?;
    let chunk = chunk.max(2);
    let mut buf = Vec::with_capacity(chunk + 1);
    let mut pos = 0u64;
    let mut lines = 0u64;
    while pos < upto {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let want = (upto - pos).min(chunk as u64);
        // A UTF-16 code unit cut by the chunk end stays in `buf` for the next.
        let before = buf.len();
        (&mut reader)
            .take(want)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| Error::io(path, e))?;
        let got = buf.len() - before;
        if got == 0 {
            break;
        }
        pos += got as u64;
        match encoding {
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let nl = encoding.newline();
                let whole = buf.len() / 2 * 2;
                lines += buf[..whole]
                    .chunks_exact(2)
                    .filter(|unit| *unit == nl)
                    .count() as u64;
                buf.drain(..whole);
            }
            _ => {
                lines += memchr::memchr_iter(b'\n', &buf).count() as u64;
                buf.clear();
            }
        }
    }
    Ok(lines)
}

// ---- open files --------------------------------------------------------------------

/// What the viewer needs to know about a file it opens.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerFile {
    /// Names this open file in [`Sessions`].
    pub session: u64,
    pub size: u64,
    pub modified_ms: Option<i64>,
    /// Guessed from the first page; the user can choose another.
    pub detected: Detected,
    /// Reads can start anywhere in the file without reading what is before.
    pub seekable: bool,
}

struct Session {
    reader: tokio::sync::Mutex<RangeReader>,
}

/// The files the viewer has open, each with its page cache, and the
/// searches and line counts running on them.
pub struct Sessions {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<Session>>>,
    running: Mutex<HashMap<u64, CancellationToken>>,
    limit: usize,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new(8)
    }
}

impl Sessions {
    /// Keep at most `limit` files open; opening another closes the oldest.
    pub fn new(limit: usize) -> Self {
        Self {
            next: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            limit: limit.max(1),
        }
    }

    /// Open `path` and look at its first page.
    pub async fn open(&self, fs: Arc<dyn FileSystem>, path: &str) -> Result<ViewerFile> {
        let entry = fs.stat(path).await?;
        if entry.is_dir() {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        let seekable = fs.seekable_reads();
        let mut reader = RangeReader::with_size(fs, path, entry.size);
        let first = reader.read(0, reader.page_size()).await?;
        let detected = detect_encoding(&first, first.len() as u64 >= entry.size);
        let session = self.next.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while open.len() >= self.limit {
            let oldest = *open.keys().min().expect("not empty");
            open.remove(&oldest);
        }
        open.insert(
            session,
            Arc::new(Session {
                reader: tokio::sync::Mutex::new(reader),
            }),
        );
        Ok(ViewerFile {
            session,
            size: entry.size,
            modified_ms: entry.modified_ms,
            detected,
            seekable,
        })
    }

    fn get(&self, session: u64) -> Result<Arc<Session>> {
        self.open
            .lock()
            .unwrap()
            .get(&session)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("the viewer file {session} is not open")))
    }

    /// Up to `len` bytes from `offset` (see [`RangeReader::read`]).
    pub async fn read(&self, session: u64, offset: u64, len: usize) -> Result<Vec<u8>> {
        let s = self.get(session)?;
        let mut reader = s.reader.lock().await;
        reader.read(offset, len).await
    }

    /// The file system and path of an open file, and its size.
    async fn target(&self, session: u64) -> Result<(Arc<dyn FileSystem>, String, u64)> {
        let s = self.get(session)?;
        let r = s.reader.lock().await;
        Ok((r.file_system().clone(), r.path().to_string(), r.size()))
    }

    fn start(&self, request: u64) -> CancellationToken {
        let cancel = CancellationToken::new();
        self.running.lock().unwrap().insert(request, cancel.clone());
        cancel
    }

    fn finish(&self, request: u64) {
        self.running.lock().unwrap().remove(&request);
    }

    /// [`search`] in an open file. Its own stream, so pages can be read
    /// while it runs. `request` names it for [`Sessions::cancel`].
    pub async fn search(
        &self,
        session: u64,
        request: u64,
        needle: &[u8],
        from: u64,
        options: SearchOptions,
        progress: impl FnMut(u64),
    ) -> Result<Option<u64>> {
        let (fs, path, size) = self.target(session).await?;
        let cancel = self.start(request);
        let result = search(
            fs.as_ref(),
            &path,
            size,
            needle,
            from,
            options,
            &cancel,
            progress,
        )
        .await;
        self.finish(request);
        result
    }

    /// [`count_lines`] in an open file.
    pub async fn count_lines(
        &self,
        session: u64,
        request: u64,
        upto: u64,
        encoding: Encoding,
    ) -> Result<u64> {
        let (fs, path, _) = self.target(session).await?;
        let cancel = self.start(request);
        let result = count_lines(fs.as_ref(), &path, upto, encoding, &cancel).await;
        self.finish(request);
        result
    }

    /// Stop search or line count `request`. Does nothing if it has finished.
    pub fn cancel(&self, request: u64) {
        if let Some(c) = self.running.lock().unwrap().remove(&request) {
            c.cancel();
        }
    }

    /// Forget an open file and its cached pages.
    pub fn close(&self, session: u64) {
        self.open.lock().unwrap().remove(&session);
    }

    /// Open files, for tests.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

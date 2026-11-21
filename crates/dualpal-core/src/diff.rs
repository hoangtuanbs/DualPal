//! The built-in file compare view: a line diff of two files on any file
//! systems, with intra-line highlights, copying changes from one side to the
//! other and saving each side back through its [`FileSystem`].
//!
//! Both files are read through the [`FileSystem`] trait, so a file on a
//! server compares like a local one without being downloaded first. Text
//! files up to [`MAX_BYTES`] and [`MAX_LINES`] are held in memory and
//! diffed; bigger ones are refused with [`DiffOutcome::TooLarge`] (an
//! external compare app can still open them). Files that are not text are
//! compared byte by byte, streamed, without a size limit.
//!
//! The encodings and their detection are the viewer's
//! ([`crate::viewer::detect_encoding`], [`crate::viewer::decode`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::vfs::{FileSystem, Reader};
use crate::viewer::{decode, detect_encoding, encode, Detected, Encoding};

/// Largest file compared line by line.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Most lines a file compared line by line may have.
pub const MAX_LINES: usize = 500_000;
/// Bytes looked at to tell text from binary.
const SNIFF: usize = 64 * 1024;
/// Bytes read per step.
const CHUNK: usize = 1 << 20;
/// Longest pair of lines (in characters) given intra-line highlights; longer
/// ones are marked as a whole.
const MAX_INTRA: usize = 20_000;
/// Time the line diff may take before it settles for a less minimal result.
const DEADLINE: Duration = Duration::from_secs(5);

// ---- options and lines ------------------------------------------------------------

/// What does not count as a difference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffOptions {
    /// Spaces and tabs anywhere in a line do not count.
    pub ignore_whitespace: bool,
    /// Upper and lower case letters are the same.
    pub ignore_case: bool,
    /// CRLF, LF and a missing line feed at the end are the same.
    pub ignore_line_endings: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// How a line ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Eol {
    /// The last line of a file without a line feed after it.
    None,
    Lf,
    CrLf,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::None => "",
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
        }
    }
}

/// The line endings of a whole file, for the status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Endings {
    /// No line feed at all (empty or one line).
    None,
    Lf,
    CrLf,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub eol: Eol,
}

impl Line {
    pub fn new(text: impl Into<String>, eol: Eol) -> Self {
        Self {
            text: text.into(),
            eol,
        }
    }
}

/// Split text into lines. A CR before a line feed belongs to the line
/// ending; a last line without a line feed has [`Eol::None`]; empty text has
/// no lines.
pub fn split_lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(nl) = rest.find('\n') {
        let line = &rest[..nl];
        out.push(match line.strip_suffix('\r') {
            Some(t) => Line::new(t, Eol::CrLf),
            None => Line::new(line, Eol::Lf),
        });
        rest = &rest[nl + 1..];
    }
    if !rest.is_empty() {
        out.push(Line::new(rest, Eol::None));
    }
    out
}

/// The text of `lines`, each followed by its line ending.
pub fn join_lines(lines: &[Line]) -> String {
    let mut s = String::with_capacity(lines.iter().map(|l| l.text.len() + 2).sum());
    for l in lines {
        s.push_str(&l.text);
        s.push_str(l.eol.as_str());
    }
    s
}

/// The line ending most lines use (LF when there are none).
fn dominant(lines: &[Line]) -> Eol {
    let crlf = lines.iter().filter(|l| l.eol == Eol::CrLf).count();
    let lf = lines.iter().filter(|l| l.eol == Eol::Lf).count();
    if crlf > lf {
        Eol::CrLf
    } else {
        Eol::Lf
    }
}

fn endings(lines: &[Line]) -> Endings {
    let crlf = lines.iter().any(|l| l.eol == Eol::CrLf);
    let lf = lines.iter().any(|l| l.eol == Eol::Lf);
    match (lf, crlf) {
        (true, true) => Endings::Mixed,
        (true, false) => Endings::Lf,
        (false, true) => Endings::CrLf,
        (false, false) => Endings::None,
    }
}

/// What a line is compared by under `options`.
fn key(line: &Line, o: &DiffOptions) -> String {
    let mut s: String = if o.ignore_whitespace {
        line.text.chars().filter(|c| !c.is_whitespace()).collect()
    } else {
        line.text.clone()
    };
    if o.ignore_case {
        s = s.to_lowercase();
    }
    if !o.ignore_line_endings {
        // Not a character a line can hold: lines never contain '\n'.
        s.push('\n');
        s.push(match line.eol {
            Eol::None => '0',
            Eol::Lf => '1',
            Eol::CrLf => '2',
        });
    }
    s
}

// ---- the line diff ----------------------------------------------------------------

/// A run of changed lines: `left_len` lines at `left_start` stand where the
/// other file has `right_len` lines at `right_start`. One side may have none
/// (lines added or removed). Line numbers count from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub left_start: u32,
    pub left_len: u32,
    pub right_start: u32,
    pub right_len: u32,
}

impl Hunk {
    pub fn start(&self, side: Side) -> usize {
        match side {
            Side::Left => self.left_start as usize,
            Side::Right => self.right_start as usize,
        }
    }
    pub fn len(&self, side: Side) -> usize {
        match side {
            Side::Left => self.left_len as usize,
            Side::Right => self.right_len as usize,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.left_len == 0 && self.right_len == 0
    }
}

/// The changes between two files' lines, in order.
pub fn diff_lines(left: &[Line], right: &[Line], options: &DiffOptions) -> Vec<Hunk> {
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut intern = |l: &Line| {
        let next = ids.len() as u32;
        *ids.entry(key(l, options)).or_insert(next)
    };
    let a: Vec<u32> = left.iter().map(&mut intern).collect();
    let b: Vec<u32> = right.iter().map(&mut intern).collect();
    let ops = similar::capture_diff_slices_deadline(
        Algorithm::Patience,
        &a,
        &b,
        Some(Instant::now() + DEADLINE),
    );
    let mut hunks: Vec<Hunk> = Vec::new();
    for op in ops {
        let (tag, l, r) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        if let Some(last) = hunks.last_mut() {
            if (last.left_start + last.left_len) as usize == l.start
                && (last.right_start + last.right_len) as usize == r.start
            {
                last.left_len += l.len() as u32;
                last.right_len += r.len() as u32;
                continue;
            }
        }
        hunks.push(Hunk {
            left_start: l.start as u32,
            left_len: l.len() as u32,
            right_start: r.start as u32,
            right_len: r.len() as u32,
        });
    }
    hunks
}

// ---- intra-line changes -------------------------------------------------------------

/// Words, runs of whitespace and single other characters, as
/// (byte start, byte end, UTF-16 start, UTF-16 end).
fn tokens(s: &str) -> Vec<(usize, usize, u32, u32)> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<(usize, usize, u32, u32)> = Vec::new();
    let mut u16_pos = 0u32;
    let mut prev: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        let w = c.len_utf16() as u32;
        let joins = k != Class::Other && prev.as_ref() == Some(&k);
        match out.last_mut() {
            Some(last) if joins => {
                last.1 = i + c.len_utf8();
                last.3 += w;
            }
            _ => out.push((i, i + c.len_utf8(), u16_pos, u16_pos + w)),
        }
        u16_pos += w;
        prev = Some(k);
    }
    out
}

/// The parts of two changed lines that differ, as [start, end) ranges of
/// UTF-16 code units (JavaScript string indexes), for the left and the
/// right line. Words are compared, so a changed word is marked whole.
pub fn intra_line(
    left: &str,
    right: &str,
    options: &DiffOptions,
) -> (Vec<[u32; 2]>, Vec<[u32; 2]>) {
    let whole = |s: &str| {
        let n = s.encode_utf16().count() as u32;
        if n == 0 {
            vec![]
        } else {
            vec![[0, n]]
        }
    };
    if left.len() + right.len() > MAX_INTRA * 4
        || left.chars().count() + right.chars().count() > MAX_INTRA
    {
        return (whole(left), whole(right));
    }
    let ta = tokens(left);
    let tb = tokens(right);
    // Whitespace that does not count is left out of the comparison.
    let keep = |s: &str, t: &(usize, usize, u32, u32)| {
        !(options.ignore_whitespace && s[t.0..t.1].chars().all(char::is_whitespace))
    };
    let ta: Vec<_> = ta.into_iter().filter(|t| keep(left, t)).collect();
    let tb: Vec<_> = tb.into_iter().filter(|t| keep(right, t)).collect();
    let norm = |s: &str, t: &(usize, usize, u32, u32)| {
        let piece = &s[t.0..t.1];
        if options.ignore_case {
            piece.to_lowercase()
        } else {
            piece.to_string()
        }
    };
    let ka: Vec<String> = ta.iter().map(|t| norm(left, t)).collect();
    let kb: Vec<String> = tb.iter().map(|t| norm(right, t)).collect();
    let ops = similar::capture_diff_slices(Algorithm::Myers, &ka, &kb);
    let mut ma: Vec<[u32; 2]> = Vec::new();
    let mut mb: Vec<[u32; 2]> = Vec::new();
    let add = |marks: &mut Vec<[u32; 2]>,
               toks: &[(usize, usize, u32, u32)],
               r: std::ops::Range<usize>| {
        if r.is_empty() {
            return;
        }
        let (a, b) = (toks[r.start].2, toks[r.end - 1].3);
        match marks.last_mut() {
            Some(last) if last[1] == a => last[1] = b,
            _ => marks.push([a, b]),
        }
    };
    for op in ops {
        let (tag, l, r) = op.as_tag_tuple();
        if tag != DiffTag::Equal {
            add(&mut ma, &ta, l);
            add(&mut mb, &tb, r);
        }
    }
    (ma, mb)
}

// ---- a compared pair of files -------------------------------------------------------

/// One side of a [`DiffDoc`].
#[derive(Clone, Debug)]
pub struct SideText {
    /// The bytes as last read or saved.
    pub original: Vec<u8>,
    pub detected: Detected,
    pub encoding: Encoding,
    /// Byte order mark length kept in front of the text.
    pub bom: usize,
    pub lines: Vec<Line>,
    /// Changed since it was read or saved.
    pub dirty: bool,
    /// Decoding and encoding again gives the same bytes, so saving changes
    /// nothing but the lines that were copied.
    pub lossless: bool,
}

impl SideText {
    /// Decode `bytes`, as `encoding` or the detected one.
    pub fn new(bytes: Vec<u8>, encoding: Option<Encoding>) -> Self {
        let detected = detect_encoding(&bytes, true);
        let mut s = Self {
            original: bytes,
            detected,
            encoding: detected.encoding,
            bom: 0,
            lines: Vec::new(),
            dirty: false,
            lossless: false,
        };
        s.decode_as(encoding.unwrap_or(detected.encoding));
        s
    }

    /// Decode the original bytes again as `encoding`. Forgets changes.
    pub fn decode_as(&mut self, encoding: Encoding) {
        self.encoding = encoding;
        self.bom = if encoding == self.detected.encoding {
            self.detected.bom as usize
        } else {
            0
        };
        let text = decode(&self.original[self.bom..], encoding);
        self.lines = split_lines(&text);
        self.dirty = false;
        self.lossless = self.encoded().as_deref() == Some(&self.original[..]);
    }

    /// The bytes to save: the BOM and the lines in the side's encoding.
    /// `None` when a character cannot be written in it.
    pub fn encoded(&self) -> Option<Vec<u8>> {
        let mut out = self.original[..self.bom].to_vec();
        out.extend(encode(&join_lines(&self.lines), self.encoding)?);
        Some(out)
    }

    /// The line endings the side uses now.
    pub fn endings(&self) -> Endings {
        endings(&self.lines)
    }

    fn info(&self, size: u64) -> SideInfo {
        SideInfo {
            encoding: self.encoding,
            detected: self.detected.encoding,
            bom: self.bom > 0,
            lines: self.lines.len() as u32,
            size,
            endings: endings(&self.lines),
            dirty: self.dirty,
            saveable: self.lossless,
        }
    }
}

/// Two files' lines and the changes between them.
#[derive(Clone, Debug)]
pub struct DiffDoc {
    pub sides: [SideText; 2],
    pub options: DiffOptions,
    pub hunks: Vec<Hunk>,
    /// Goes up with every change, so a stale request is refused.
    pub revision: u64,
}

impl DiffDoc {
    pub fn new(left: SideText, right: SideText, options: DiffOptions) -> Self {
        let mut d = Self {
            sides: [left, right],
            options,
            hunks: Vec::new(),
            revision: 1,
        };
        d.recompute();
        d
    }

    /// A pair of texts, for tests.
    pub fn from_texts(left: &str, right: &str, options: DiffOptions) -> Self {
        Self::new(
            SideText::new(left.as_bytes().to_vec(), None),
            SideText::new(right.as_bytes().to_vec(), None),
            options,
        )
    }

    pub fn side(&self, side: Side) -> &SideText {
        &self.sides[side.index()]
    }

    /// Diff the lines again.
    pub fn recompute(&mut self) {
        self.hunks = diff_lines(&self.sides[0].lines, &self.sides[1].lines, &self.options);
        self.revision += 1;
    }

    pub fn set_options(&mut self, options: DiffOptions) {
        self.options = options;
        self.recompute();
    }

    /// Read one side's bytes as another encoding. Refused while it has
    /// changes that are not saved.
    pub fn set_encoding(&mut self, side: Side, encoding: Encoding) -> Result<()> {
        let s = &mut self.sides[side.index()];
        if s.encoding == encoding {
            return Ok(());
        }
        if s.dirty {
            return Err(Error::Invalid(
                "save or discard the changes before choosing another encoding".into(),
            ));
        }
        s.decode_as(encoding);
        self.recompute();
        Ok(())
    }

    /// Both files have the same lines and line endings.
    pub fn identical(&self) -> bool {
        self.sides[0].lines == self.sides[1].lines
    }

    /// Copy change `hunk` to side `to`: its lines there become the other
    /// side's. `revision` must be the current one.
    pub fn apply(&mut self, revision: u64, hunk: usize, to: Side) -> Result<()> {
        if revision != self.revision {
            return Err(Error::Invalid(
                "the compare view is out of date; try again".into(),
            ));
        }
        let h = *self
            .hunks
            .get(hunk)
            .ok_or_else(|| Error::Invalid(format!("there is no change {hunk}")))?;
        let from = to.other();
        let src = &self.sides[from.index()].lines;
        let mut copied: Vec<Line> = src[h.start(from)..h.start(from) + h.len(from)].to_vec();
        let target = &mut self.sides[to.index()];
        let eol = dominant(&target.lines);
        if self.options.ignore_line_endings {
            // Line endings do not count: copied lines take the file's own.
            for l in &mut copied {
                if l.eol != Eol::None {
                    l.eol = eol;
                }
            }
        }
        let at = h.start(to);
        target.lines.splice(at..at + h.len(to), copied);
        // Only the last line may go without a line feed.
        let n = target.lines.len();
        for l in target.lines.iter_mut().take(n.saturating_sub(1)) {
            if l.eol == Eol::None {
                l.eol = eol;
            }
        }
        target.dirty = true;
        self.recompute();
        Ok(())
    }

    /// Lines `from..from + count` of `side` with their intra-line changes.
    pub fn lines(&self, side: Side, from: usize, count: usize) -> Vec<DiffLine> {
        let lines = &self.side(side).lines;
        let other = &self.side(side.other()).lines;
        let end = lines.len().min(from.saturating_add(count));
        (from.min(end)..end)
            .map(|i| {
                let line = &lines[i];
                let mut marks = Vec::new();
                let k = self
                    .hunks
                    .partition_point(|h| h.start(side) + h.len(side) <= i);
                if let Some(h) = self.hunks.get(k) {
                    let pos = i.wrapping_sub(h.start(side));
                    if i >= h.start(side) && pos < h.len(side) && pos < h.len(side.other()) {
                        let o = &other[h.start(side.other()) + pos];
                        let (l, r) = match side {
                            Side::Left => (&line.text, &o.text),
                            Side::Right => (&o.text, &line.text),
                        };
                        let (ml, mr) = intra_line(l, r, &self.options);
                        marks = if side == Side::Left { ml } else { mr };
                    }
                }
                DiffLine {
                    text: line.text.clone(),
                    eol: line.eol,
                    marks,
                }
            })
            .collect()
    }
}

// ---- what the UI gets ---------------------------------------------------------------

/// One side, for the toolbar and the status bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideInfo {
    pub encoding: Encoding,
    pub detected: Encoding,
    pub bom: bool,
    pub lines: u32,
    pub size: u64,
    pub endings: Endings,
    pub dirty: bool,
    /// Saving keeps every byte that was not changed.
    pub saveable: bool,
}

/// A compared pair as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffState {
    pub session: u64,
    pub revision: u64,
    pub options: DiffOptions,
    pub hunks: Vec<Hunk>,
    /// Same lines and line endings, whatever the options.
    pub identical: bool,
    pub sides: [SideInfo; 2],
}

/// A line of one side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub text: String,
    pub eol: Eol,
    /// Changed parts, as [start, end) UTF-16 code unit ranges of `text`.
    pub marks: Vec<[u32; 2]>,
}

/// What opening a compare found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DiffOutcome {
    /// Two text files, compared line by line.
    Text { state: DiffState },
    /// At least one file is not text; compared byte by byte.
    Binary { identical: bool, sizes: [u64; 2] },
    /// Too big to compare line by line here.
    TooLarge {
        sizes: [u64; 2],
        max_bytes: u64,
        max_lines: u64,
    },
}

// ---- reading ------------------------------------------------------------------------

/// Read up to `max` bytes of `path`; `None` if it is longer.
async fn read_all(
    fs: &dyn FileSystem,
    path: &str,
    max: u64,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>> {
    let mut r = fs.open_read(path).await?;
    let mut buf = Vec::new();
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let got = fill(&mut r, &mut buf, CHUNK, path).await?;
        if buf.len() as u64 > max {
            return Ok(None);
        }
        if got < CHUNK {
            return Ok(Some(buf));
        }
    }
}

/// Append up to `want` bytes from `r` to `buf`; fewer only at the end.
async fn fill(r: &mut Reader, buf: &mut Vec<u8>, want: usize, path: &str) -> Result<usize> {
    let before = buf.len();
    r.take(want as u64)
        .read_to_end(buf)
        .await
        .map_err(|e| Error::io(path, e))?;
    Ok(buf.len() - before)
}

/// Whether two files have the same bytes, streaming both.
pub async fn same_bytes(
    a: (&dyn FileSystem, &str),
    b: (&dyn FileSystem, &str),
    cancel: &CancellationToken,
) -> Result<bool> {
    let mut ra = a.0.open_read(a.1).await?;
    let mut rb = b.0.open_read(b.1).await?;
    let (mut ba, mut bb) = (Vec::with_capacity(SNIFF), Vec::with_capacity(SNIFF));
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        ba.clear();
        bb.clear();
        let na = fill(&mut ra, &mut ba, SNIFF, a.1).await?;
        let nb = fill(&mut rb, &mut bb, SNIFF, b.1).await?;
        if ba != bb {
            return Ok(false);
        }
        if na < SNIFF || nb < SNIFF {
            return Ok(na == nb);
        }
    }
}

/// Compare `left` and `right`: line by line when both are text and small
/// enough, else byte by byte or not at all (see [`DiffOutcome`]).
pub async fn compare(
    left: (&dyn FileSystem, &str),
    right: (&dyn FileSystem, &str),
    options: DiffOptions,
    encodings: [Option<Encoding>; 2],
    cancel: &CancellationToken,
) -> Result<std::result::Result<(DiffDoc, [Meta; 2]), DiffOutcome>> {
    let mut metas = Vec::new();
    let mut binary = false;
    for (fs, path) in [left, right] {
        let entry = fs.stat(path).await?;
        if entry.is_dir() {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        let head = fs.read_range(path, 0, SNIFF).await?;
        binary |= detect_encoding(&head, head.len() as u64 >= entry.size).binary;
        metas.push(Meta {
            size: entry.size,
            modified_ms: entry.modified_ms,
        });
    }
    let sizes = [metas[0].size, metas[1].size];
    if binary {
        let identical = sizes[0] == sizes[1] && same_bytes(left, right, cancel).await?;
        return Ok(Err(DiffOutcome::Binary { identical, sizes }));
    }
    let too_large = DiffOutcome::TooLarge {
        sizes,
        max_bytes: MAX_BYTES,
        max_lines: MAX_LINES as u64,
    };
    if sizes.iter().any(|&s| s > MAX_BYTES) {
        return Ok(Err(too_large));
    }
    let mut texts = Vec::new();
    for (i, (fs, path)) in [left, right].into_iter().enumerate() {
        let Some(bytes) = read_all(fs, path, MAX_BYTES, cancel).await? else {
            return Ok(Err(too_large));
        };
        metas[i].size = bytes.len() as u64;
        let side = SideText::new(bytes, encodings[i]);
        if side.lines.len() > MAX_LINES {
            return Ok(Err(too_large));
        }
        texts.push(side);
    }
    let right_text = texts.pop().expect("two sides");
    let left_text = texts.pop().expect("two sides");
    let doc = tokio::task::spawn_blocking(move || DiffDoc::new(left_text, right_text, options))
        .await
        .map_err(|e| Error::Invalid(format!("comparing failed: {e}")))?;
    Ok(Ok((doc, [metas[0].clone(), metas[1].clone()])))
}

/// What a side's file looked like when it was read or saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub size: u64,
    pub modified_ms: Option<i64>,
}

// ---- open compares ------------------------------------------------------------------

struct Session {
    files: [(Arc<dyn FileSystem>, String); 2],
    meta: [Meta; 2],
    doc: DiffDoc,
}

/// The compares the UI has open.
pub struct DiffSessions {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<tokio::sync::Mutex<Session>>>>,
    running: Mutex<HashMap<u64, CancellationToken>>,
    limit: usize,
}

impl Default for DiffSessions {
    fn default() -> Self {
        Self::new(4)
    }
}

impl DiffSessions {
    /// Keep at most `limit` compares open; opening another closes the oldest.
    pub fn new(limit: usize) -> Self {
        Self {
            next: AtomicU64::new(1),
            open: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            limit: limit.max(1),
        }
    }

    /// Compare two files. `request` names the work for [`DiffSessions::cancel`].
    pub async fn open(
        &self,
        request: u64,
        left: (Arc<dyn FileSystem>, String),
        right: (Arc<dyn FileSystem>, String),
        options: DiffOptions,
        encodings: [Option<Encoding>; 2],
    ) -> Result<DiffOutcome> {
        let cancel = CancellationToken::new();
        self.running.lock().unwrap().insert(request, cancel.clone());
        let result = compare(
            (left.0.as_ref(), &left.1),
            (right.0.as_ref(), &right.1),
            options,
            encodings,
            &cancel,
        )
        .await;
        self.running.lock().unwrap().remove(&request);
        let (doc, meta) = match result? {
            Ok(found) => found,
            Err(outcome) => return Ok(outcome),
        };
        let session = self.next.fetch_add(1, Ordering::SeqCst);
        let state = state_of(session, &doc, &meta);
        let mut open = self.open.lock().unwrap();
        while open.len() >= self.limit {
            let oldest = *open.keys().min().expect("not empty");
            open.remove(&oldest);
        }
        open.insert(
            session,
            Arc::new(tokio::sync::Mutex::new(Session {
                files: [left, right],
                meta,
                doc,
            })),
        );
        Ok(DiffOutcome::Text { state })
    }

    fn get(&self, session: u64) -> Result<Arc<tokio::sync::Mutex<Session>>> {
        self.open
            .lock()
            .unwrap()
            .get(&session)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("the compare {session} is not open")))
    }

    /// The current state of an open compare.
    pub async fn state(&self, session: u64) -> Result<DiffState> {
        let s = self.get(session)?;
        let s = s.lock().await;
        Ok(state_of(session, &s.doc, &s.meta))
    }

    /// Lines of one side with their intra-line changes.
    pub async fn lines(
        &self,
        session: u64,
        side: Side,
        from: usize,
        count: usize,
    ) -> Result<Vec<DiffLine>> {
        let s = self.get(session)?;
        let s = s.lock().await;
        Ok(s.doc.lines(side, from, count))
    }

    /// Run `f` on the compare off the async threads (diffs can take a while).
    async fn update(
        &self,
        session: u64,
        f: impl FnOnce(&mut DiffDoc) -> Result<()> + Send + 'static,
    ) -> Result<DiffState> {
        let s = self.get(session)?;
        let mut guard = s.lock_owned().await;
        let (guard, result) = tokio::task::spawn_blocking(move || {
            let r = f(&mut guard.doc);
            (guard, r)
        })
        .await
        .map_err(|e| Error::Invalid(format!("comparing failed: {e}")))?;
        result?;
        Ok(state_of(session, &guard.doc, &guard.meta))
    }

    pub async fn set_options(&self, session: u64, options: DiffOptions) -> Result<DiffState> {
        self.update(session, move |d| {
            d.set_options(options);
            Ok(())
        })
        .await
    }

    pub async fn set_encoding(
        &self,
        session: u64,
        side: Side,
        encoding: Encoding,
    ) -> Result<DiffState> {
        self.update(session, move |d| d.set_encoding(side, encoding))
            .await
    }

    /// Copy change `hunk` to side `to` (see [`DiffDoc::apply`]).
    pub async fn apply(
        &self,
        session: u64,
        revision: u64,
        hunk: usize,
        to: Side,
    ) -> Result<DiffState> {
        self.update(session, move |d| d.apply(revision, hunk, to))
            .await
    }

    /// Write one side back to its file. Refused when the file changed since
    /// it was read, or when its text cannot be written in its encoding.
    pub async fn save(&self, session: u64, side: Side) -> Result<DiffState> {
        let s = self.get(session)?;
        let mut s = s.lock().await;
        let i = side.index();
        let (fs, path) = s.files[i].clone();
        let name = fs.file_name(&path).unwrap_or_else(|| path.clone());
        let text = &s.doc.sides[i];
        if !text.lossless {
            return Err(Error::Unsupported(format!(
                "saving {name}: it does not read back the same in {}; choose its encoding first",
                encoding_name(text.encoding)
            )));
        }
        let bytes = text.encoded().ok_or_else(|| {
            Error::Invalid(format!(
                "saving {name}: some characters cannot be written in {}",
                encoding_name(text.encoding)
            ))
        })?;
        let now = fs.stat(&path).await?;
        let was = &s.meta[i];
        if now.size != was.size || (was.modified_ms.is_some() && now.modified_ms != was.modified_ms)
        {
            return Err(Error::Invalid(format!(
                "{name} changed since it was compared; compare again before saving"
            )));
        }
        crate::vfs::write_bytes(fs.as_ref(), &path, &bytes).await?;
        let after = fs.stat(&path).await.ok();
        s.meta[i] = Meta {
            size: bytes.len() as u64,
            modified_ms: after.and_then(|e| e.modified_ms),
        };
        let t = &mut s.doc.sides[i];
        t.original = bytes;
        t.dirty = false;
        t.lossless = true;
        Ok(state_of(session, &s.doc, &s.meta))
    }

    /// Stop opening `request`. Does nothing if it has finished.
    pub fn cancel(&self, request: u64) {
        if let Some(c) = self.running.lock().unwrap().remove(&request) {
            c.cancel();
        }
    }

    /// Forget an open compare.
    pub fn close(&self, session: u64) {
        self.open.lock().unwrap().remove(&session);
    }

    /// Open compares, for tests.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Utf8 => "UTF-8",
        Encoding::Utf16Le => "UTF-16 LE",
        Encoding::Utf16Be => "UTF-16 BE",
        Encoding::Windows1252 => "Windows-1252",
        Encoding::Latin1 => "Latin-1",
    }
}

fn state_of(session: u64, doc: &DiffDoc, meta: &[Meta; 2]) -> DiffState {
    DiffState {
        session,
        revision: doc.revision,
        options: doc.options,
        hunks: doc.hunks.clone(),
        identical: doc.identical(),
        sides: [
            doc.sides[0].info(meta[0].size),
            doc.sides[1].info(meta[1].size),
        ],
    }
}

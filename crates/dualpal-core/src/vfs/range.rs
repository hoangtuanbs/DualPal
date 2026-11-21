//! Reading byte ranges of a file on any [`FileSystem`], for the file viewer
//! (and the file compare view built on it).
//!
//! [`FileSystem::read_range`] reads one range. [`RangeReader`] reads a file
//! in fixed pages and keeps the most recently used ones, so scrolling back
//! and forth over a big file reads each part once. On backends that cannot
//! start a read at an offset (see [`FileSystem::seekable_reads`]) it keeps
//! one stream open and reads on from where it is, caching the pages it
//! passes, instead of streaming the file from the start for every page.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::AsyncReadExt;

use super::{FileSystem, Reader};
use crate::error::{Error, Result};

/// Bytes per page of a [`RangeReader`].
pub const PAGE_SIZE: usize = 64 * 1024;
/// Pages a [`RangeReader`] keeps by default (8 MiB).
pub const DEFAULT_CACHE_PAGES: usize = 128;

/// Read `want` bytes from `reader`, or fewer if it ends first.
pub(crate) async fn read_up_to(mut reader: Reader, want: u64, path: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(want.min(16 << 20) as usize);
    (&mut reader)
        .take(want)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// What a [`RangeReader`] has done so far, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadStats {
    /// Streams opened on the file system.
    pub opens: u64,
    /// Bytes read from the file system (cache misses, including bytes
    /// passed over on backends without seeking).
    pub bytes_read: u64,
    /// Pages served from the cache.
    pub hits: u64,
}

/// A paged, cached reader over one file. See the module docs.
pub struct RangeReader {
    fs: Arc<dyn FileSystem>,
    path: String,
    size: u64,
    page_size: usize,
    capacity: usize,
    /// Page index → (bytes, last use).
    pages: HashMap<u64, (Arc<[u8]>, u64)>,
    tick: u64,
    /// An open stream and the offset it will read next.
    stream: Option<(u64, Reader)>,
    stats: ReadStats,
}

impl RangeReader {
    /// A reader over `path`, whose size is read with `stat`.
    pub async fn open(fs: Arc<dyn FileSystem>, path: &str) -> Result<Self> {
        let entry = fs.stat(path).await?;
        if entry.is_dir() {
            return Err(Error::Invalid(format!("not a file: {path}")));
        }
        Ok(Self::with_size(fs, path, entry.size))
    }

    /// A reader over `path` when its size is already known.
    pub fn with_size(fs: Arc<dyn FileSystem>, path: &str, size: u64) -> Self {
        Self {
            fs,
            path: path.to_string(),
            size,
            page_size: PAGE_SIZE,
            capacity: DEFAULT_CACHE_PAGES,
            pages: HashMap::new(),
            tick: 0,
            stream: None,
            stats: ReadStats::default(),
        }
    }

    /// Use pages of `page_size` bytes and keep at most `pages` of them.
    /// Clears the cache.
    pub fn with_pages(mut self, page_size: usize, pages: usize) -> Self {
        self.page_size = page_size.max(1);
        self.capacity = pages.max(1);
        self.pages.clear();
        self.stream = None;
        self
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn file_system(&self) -> &Arc<dyn FileSystem> {
        &self.fs
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn stats(&self) -> ReadStats {
        self.stats
    }

    /// Pages in the cache now.
    pub fn cached_pages(&self) -> usize {
        self.pages.len()
    }

    /// Read up to `len` bytes from `offset`. Shorter at the end of the
    /// file, empty from the end on.
    pub async fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len == 0 || offset >= self.size {
            return Ok(Vec::new());
        }
        let end = self.size.min(offset.saturating_add(len as u64));
        let ps = self.page_size as u64;
        let mut out = Vec::with_capacity((end - offset) as usize);
        let mut index = offset / ps;
        while index * ps < end {
            let page = self.page(index).await?;
            let start = index * ps;
            let from = offset.saturating_sub(start) as usize;
            let to = ((end - start) as usize).min(page.len());
            if from >= to {
                break; // The file is shorter than it was.
            }
            out.extend_from_slice(&page[from..to]);
            if page.len() < self.page_size {
                break;
            }
            index += 1;
        }
        Ok(out)
    }

    /// The bytes of page `index` (shorter for the last page, empty past it).
    pub async fn page(&mut self, index: u64) -> Result<Arc<[u8]>> {
        self.tick += 1;
        if let Some((bytes, used)) = self.pages.get_mut(&index) {
            *used = self.tick;
            self.stats.hits += 1;
            return Ok(bytes.clone());
        }
        let start = index * self.page_size as u64;
        if start >= self.size {
            return Ok(Arc::from(Vec::new()));
        }
        let result = self.fetch(index).await;
        if result.is_err() {
            // A failed stream is not reused; the next read opens a new one.
            self.stream = None;
        }
        result
    }

    async fn fetch(&mut self, index: u64) -> Result<Arc<[u8]>> {
        let ps = self.page_size as u64;
        let start = index * ps;
        let pos = self.stream.as_ref().map(|(p, _)| *p);
        let reuse = match pos {
            Some(p) if p == start => true,
            // Without seeking, reading on is cheaper than starting over.
            Some(p) if p < start && !self.fs.seekable_reads() => true,
            _ => false,
        };
        if !reuse {
            let from = if self.fs.seekable_reads() { start } else { 0 };
            let reader = self.fs.open_read_at(&self.path, from).await?;
            self.stats.opens += 1;
            self.stream = Some((from, reader));
        }
        // Read pages up to and including `index`, caching each one passed.
        loop {
            let (p, _) = self.stream.as_ref().expect("stream is open");
            let here = *p / ps;
            let bytes = self.read_page_from_stream().await?;
            let last = bytes.len() < self.page_size;
            self.insert(here, bytes.clone());
            if here >= index || last {
                if last {
                    self.stream = None;
                }
                return Ok(if here == index {
                    bytes
                } else {
                    Arc::from(Vec::new())
                });
            }
        }
    }

    /// Read one page from the open stream, which sits at a page start.
    async fn read_page_from_stream(&mut self) -> Result<Arc<[u8]>> {
        let (pos, reader) = self.stream.as_mut().expect("stream is open");
        let want = (self.size.saturating_sub(*pos)).min(self.page_size as u64);
        let mut buf = Vec::with_capacity(want as usize);
        reader
            .take(want)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| Error::io(&self.path, e))?;
        *pos += buf.len() as u64;
        self.stats.bytes_read += buf.len() as u64;
        Ok(Arc::from(buf))
    }

    fn insert(&mut self, index: u64, bytes: Arc<[u8]>) {
        self.tick += 1;
        self.pages.insert(index, (bytes, self.tick));
        while self.pages.len() > self.capacity {
            let oldest = self
                .pages
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| *k)
                .expect("cache is not empty");
            self.pages.remove(&oldest);
        }
    }
}

//! The file viewer's core: range reads, the page cache, encodings, streamed
//! search and line counts (VIEW-*).

use std::sync::Arc;
use std::time::{Duration, Instant};

use dualpal_core::vfs::{FileSystem, LocalFs, RangeReader};
use dualpal_core::viewer::{
    count_lines_chunked, decode, detect_encoding, encode, search, Encoding, SearchOptions, Sessions,
};
use dualpal_core::{CancellationToken, Error};
use dualpal_testkit::fixture::pseudo_random;
use dualpal_testkit::{FaultOp, FaultyFs, MemFs};

fn local_file(data: &[u8]) -> (tempfile::TempDir, Arc<dyn FileSystem>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("f.bin");
    std::fs::write(&path, data).unwrap();
    (
        tmp,
        Arc::new(LocalFs::new()),
        path.to_string_lossy().into_owned(),
    )
}

/// A memory file behind a fault injector; `seekable: false` makes reads at
/// an offset stream from the start, like a backend without seeking.
fn mem_file(data: &[u8], seekable: bool) -> (Arc<FaultyFs<MemFs>>, String) {
    let mem = MemFs::with_default_id();
    mem.seed_file("/f.bin", data);
    let fs = Arc::new(FaultyFs::new(mem));
    fs.set_seekable(seekable);
    (fs, "/f.bin".to_string())
}

/// VIEW-001 — range reads on the local disk: inside, across pages, cut at
/// the end, past the end, and an empty file; reading again uses the cache.
#[tokio::test]
async fn range_reader_local_ranges() {
    let data = pseudo_random(300_000, 1);
    let (_tmp, fs, path) = local_file(&data);
    let mut r = RangeReader::open(fs.clone(), &path).await.unwrap();
    assert_eq!(r.size(), 300_000);
    assert_eq!(r.read(0, 100).await.unwrap(), &data[..100]);
    // 65 536 is a page boundary.
    assert_eq!(r.read(65_000, 2_000).await.unwrap(), &data[65_000..67_000]);
    assert_eq!(r.read(299_000, 5_000).await.unwrap(), &data[299_000..]);
    assert!(r.read(300_000, 10).await.unwrap().is_empty());
    assert!(r.read(9_999_999, 10).await.unwrap().is_empty());
    assert!(r.read(10, 0).await.unwrap().is_empty());
    let opens = r.stats().opens;
    assert_eq!(r.read(65_000, 2_000).await.unwrap(), &data[65_000..67_000]);
    assert_eq!(r.stats().opens, opens, "cached pages are not read again");
    assert!(r.stats().hits >= 2);

    // The trait's one-off read agrees.
    assert_eq!(
        fs.read_range(&path, 65_000, 2_000).await.unwrap(),
        &data[65_000..67_000]
    );
    assert!(fs.read_range(&path, 400_000, 10).await.unwrap().is_empty());

    let (_t2, fs2, empty) = local_file(b"");
    let mut e = RangeReader::open(fs2.clone(), &empty).await.unwrap();
    assert!(e.read(0, 10).await.unwrap().is_empty());
    assert!(fs2.read_range(&empty, 0, 10).await.unwrap().is_empty());
}

/// VIEW-002 — without seeking, pages are read from one stream that moves
/// on; pages it passes are cached, and only going back reopens the file.
#[tokio::test]
async fn range_reader_streams_without_seek() {
    let data = pseudo_random(10 * 1000 + 7, 2);
    let (fs, path) = mem_file(&data, false);
    let dynfs: Arc<dyn FileSystem> = fs.clone();
    let mut r = RangeReader::open(dynfs, &path)
        .await
        .unwrap()
        .with_pages(1000, 64);
    assert_eq!(r.read(0, 1500).await.unwrap(), &data[..1500]);
    assert_eq!(r.read(1500, 1500).await.unwrap(), &data[1500..3000]);
    // Skip ahead: read on from the open stream.
    assert_eq!(r.read(7_200, 100).await.unwrap(), &data[7_200..7_300]);
    assert_eq!(fs.read_count(), 1, "one stream for reading forwards");
    // The skipped pages were cached on the way.
    assert_eq!(r.read(4_000, 2_000).await.unwrap(), &data[4_000..6_000]);
    assert_eq!(fs.read_count(), 1);
    assert_eq!(r.read(9_990, 100).await.unwrap(), &data[9_990..]);
    assert_eq!(r.stats().bytes_read, data.len() as u64, "every byte once");
    assert_eq!(fs.read_count(), 1);
    assert!(r.read(10_007, 1).await.unwrap().is_empty());

    // With seeking, a far page is read on its own.
    let (sfs, spath) = mem_file(&data, true);
    let dynfs: Arc<dyn FileSystem> = sfs.clone();
    let mut s = RangeReader::open(dynfs, &spath)
        .await
        .unwrap()
        .with_pages(1000, 64);
    assert_eq!(s.read(9_000, 10).await.unwrap(), &data[9_000..9_010]);
    assert_eq!(s.stats().bytes_read, 1000);
}

/// VIEW-003 — a failed read is reported and the next read works; the
/// cache keeps at most its page limit.
#[tokio::test]
async fn range_reader_faults_and_eviction() {
    let data = pseudo_random(20_000, 3);
    let (fs, path) = mem_file(&data, true);
    let dynfs: Arc<dyn FileSystem> = fs.clone();
    let mut r = RangeReader::open(dynfs, &path)
        .await
        .unwrap()
        .with_pages(1000, 4);
    fs.fail_next(FaultOp::OpenReadAt, Error::Connection("dropped".into()));
    assert!(matches!(r.read(5_000, 10).await, Err(Error::Connection(_))));
    assert_eq!(r.read(5_000, 10).await.unwrap(), &data[5_000..5_010]);
    for i in 0..20 {
        r.read(i * 1000, 1).await.unwrap();
        assert!(r.cached_pages() <= 4);
    }
    // The newest pages are the ones kept.
    let before = r.stats().opens;
    r.read(19_000, 10).await.unwrap();
    assert_eq!(r.stats().opens, before);
}

/// VIEW-004 — encoding detection: BOMs, UTF-16 without one, UTF-8 (also cut
/// at the end of the sample), the Windows-1252 fallback and binary data.
#[test]
fn encoding_detection() {
    let d = |b: &[u8]| detect_encoding(b, true);
    assert_eq!(d(b"\xEF\xBB\xBFhi").encoding, Encoding::Utf8);
    assert_eq!(d(b"\xEF\xBB\xBFhi").bom, 3);
    assert_eq!(d(b"\xFF\xFEh\0i\0").encoding, Encoding::Utf16Le);
    assert_eq!(d(b"\xFF\xFEh\0i\0").bom, 2);
    assert_eq!(d(b"\xFE\xFF\0h\0i").encoding, Encoding::Utf16Be);
    let le = encode("hello world\n", Encoding::Utf16Le).unwrap();
    assert_eq!(d(&le).encoding, Encoding::Utf16Le);
    assert_eq!(d(&le).bom, 0);
    assert!(!d(&le).binary);
    let be = encode("hello world\n", Encoding::Utf16Be).unwrap();
    assert_eq!(d(&be).encoding, Encoding::Utf16Be);
    assert_eq!(d("grüße €".as_bytes()).encoding, Encoding::Utf8);
    assert_eq!(d(b"plain ascii").encoding, Encoding::Utf8);
    // "é" is two bytes; the sample ends after the first.
    let cut = &"café".as_bytes()[..4];
    assert_eq!(detect_encoding(cut, false).encoding, Encoding::Utf8);
    assert_eq!(detect_encoding(cut, true).encoding, Encoding::Windows1252);
    assert_eq!(d(b"caf\xE9 cr\xE8me").encoding, Encoding::Windows1252);
    let bin = d(b"\x7FELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0>\0\x01\0\0\0\xa0\x1f");
    assert!(bin.binary);
    assert!(!d(b"").binary);
    assert_eq!(d(b"").encoding, Encoding::Utf8);
}

/// VIEW-005 — decoding and encoding in each encoding.
#[test]
fn decode_and_encode() {
    assert_eq!(decode(b"caf\xE9 \x80", Encoding::Windows1252), "café €");
    assert_eq!(decode(b"caf\xE9 \x80", Encoding::Latin1), "café \u{80}");
    assert_eq!(decode("ok ✓".as_bytes(), Encoding::Utf8), "ok ✓");
    assert_eq!(decode(b"a\xFFb", Encoding::Utf8), "a\u{FFFD}b");
    for enc in [
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Windows1252,
        Encoding::Latin1,
    ] {
        let text = "Grüße, café";
        assert_eq!(decode(&encode(text, enc).unwrap(), enc), text, "{enc:?}");
    }
    assert_eq!(encode("€", Encoding::Windows1252), Some(vec![0x80]));
    assert_eq!(encode("€", Encoding::Latin1), None);
    assert_eq!(encode("✓", Encoding::Windows1252), None);
    assert_eq!(
        decode(&encode("😀", Encoding::Utf16Le).unwrap(), Encoding::Utf16Le),
        "😀"
    );
    // Serialized with the labels TextDecoder takes.
    assert_eq!(
        serde_json::to_value(Encoding::Utf16Le).unwrap(),
        serde_json::json!("utf-16le")
    );
}

/// VIEW-006 — streamed search finds a match at every position, including
/// across chunk boundaries, forwards and backwards, with and without seek,
/// and without case.
#[tokio::test]
async fn search_across_chunk_boundaries() {
    let cancel = CancellationToken::new();
    let filler = vec![b'.'; 300];
    for at in [0usize, 1, 15, 16, 17, 31, 32, 33, 100, 297] {
        let mut data = filler.clone();
        data[at..at + 3].copy_from_slice(b"abc");
        for seekable in [true, false] {
            let (fs, path) = mem_file(&data, seekable);
            let size = data.len() as u64;
            let opts = |backward, ignore_case| SearchOptions {
                backward,
                ignore_case,
                chunk: 16,
                ..Default::default()
            };
            let find = |needle: &'static [u8], from: u64, o: SearchOptions| {
                let (fs, path, cancel) = (fs.clone(), path.clone(), cancel.clone());
                async move {
                    search(fs.as_ref(), &path, size, needle, from, o, &cancel, |_| {})
                        .await
                        .unwrap()
                }
            };
            let at = at as u64;
            assert_eq!(find(b"abc", 0, opts(false, false)).await, Some(at));
            assert_eq!(find(b"abc", at, opts(false, false)).await, Some(at));
            assert_eq!(find(b"abc", at + 1, opts(false, false)).await, None);
            assert_eq!(find(b"ABC", 0, opts(false, true)).await, Some(at));
            assert_eq!(find(b"ABC", 0, opts(false, false)).await, None);
            assert_eq!(find(b"abc", size, opts(true, false)).await, Some(at));
            assert_eq!(find(b"abc", at + 1, opts(true, false)).await, Some(at));
            assert_eq!(find(b"abc", at, opts(true, false)).await, None);
            assert_eq!(find(b"aBc", size, opts(true, true)).await, Some(at));
        }
    }
    // Several matches: next and previous step through them.
    let data = b"xx needle yy needle zz needle".to_vec();
    let (fs, path) = mem_file(&data, true);
    let size = data.len() as u64;
    let mut hits = Vec::new();
    let mut from = 0;
    let o = SearchOptions {
        chunk: 8,
        ..Default::default()
    };
    while let Some(at) = search(
        fs.as_ref(),
        &path,
        size,
        b"needle",
        from,
        o,
        &cancel,
        |_| {},
    )
    .await
    .unwrap()
    {
        hits.push(at);
        from = at + 1;
    }
    assert_eq!(hits, vec![3, 13, 23]);
    let back = SearchOptions {
        backward: true,
        ..o
    };
    let prev = search(
        fs.as_ref(),
        &path,
        size,
        b"needle",
        23,
        back,
        &cancel,
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(prev, Some(13));
    // An empty needle or one longer than the file finds nothing.
    let none = search(fs.as_ref(), &path, size, b"", 0, o, &cancel, |_| {}).await;
    assert_eq!(none.unwrap(), None);
}

/// VIEW-007 — a search can be cancelled, and reports progress.
#[tokio::test]
async fn search_cancel_and_progress() {
    let data = vec![b'a'; 100_000];
    let (fs, path) = mem_file(&data, true);
    let cancel = CancellationToken::new();
    let mut seen = Vec::new();
    let o = SearchOptions {
        chunk: 10_000,
        ..Default::default()
    };
    let r = search(fs.as_ref(), &path, 100_000, b"zz", 0, o, &cancel, |n| {
        seen.push(n)
    })
    .await
    .unwrap();
    assert_eq!(r, None);
    assert_eq!(seen.last(), Some(&100_000));
    assert!(seen.len() >= 10);
    cancel.cancel();
    let r = search(fs.as_ref(), &path, 100_000, b"zz", 0, o, &cancel, |_| {}).await;
    assert!(matches!(r, Err(Error::Cancelled)));
}

/// VIEW-008 — line counts, with chunks that cut UTF-16 code units.
#[tokio::test]
async fn line_counts() {
    let cancel = CancellationToken::new();
    let text = "one\ntwo\r\nthree\n\nfive";
    for (enc, chunk) in [
        (Encoding::Utf8, 3),
        (Encoding::Utf16Le, 3),
        (Encoding::Utf16Be, 5),
    ] {
        let data = encode(text, enc).unwrap();
        let (fs, path) = mem_file(&data, true);
        let unit = enc.unit() as u64;
        let count = |upto: u64| {
            let (fs, path, cancel) = (fs.clone(), path.clone(), cancel.clone());
            async move {
                count_lines_chunked(fs.as_ref(), &path, upto, enc, &cancel, chunk)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(count(0).await, 0, "{enc:?}");
        assert_eq!(count(4 * unit).await, 1, "{enc:?}");
        assert_eq!(count(data.len() as u64).await, 4, "{enc:?}");
        assert_eq!(count(u64::MAX).await, 4, "{enc:?}");
    }
    cancel.cancel();
    let (fs, path) = mem_file(b"a\nb\n", true);
    let r = count_lines_chunked(fs.as_ref(), &path, 4, Encoding::Utf8, &cancel, 1).await;
    assert!(matches!(r, Err(Error::Cancelled)));
}

/// VIEW-009 — open files: detection on open, page reads, the open-file
/// limit, unknown and closed files, search and cancel by request.
#[tokio::test]
async fn sessions_open_read_close() {
    let sessions = Sessions::new(2);
    let (_t, fs, path) = local_file(b"\xFF\xFEh\0i\0");
    let a = sessions.open(fs.clone(), &path).await.unwrap();
    assert_eq!(a.size, 6);
    assert_eq!(a.detected.encoding, Encoding::Utf16Le);
    assert_eq!(a.detected.bom, 2);
    assert!(a.seekable);
    assert_eq!(sessions.read(a.session, 2, 100).await.unwrap(), b"h\0i\0");
    let found = sessions
        .search(a.session, 1, b"i\0", 0, SearchOptions::default(), |_| {})
        .await
        .unwrap();
    assert_eq!(found, Some(4));
    assert_eq!(
        sessions
            .count_lines(a.session, 2, 6, Encoding::Utf16Le)
            .await
            .unwrap(),
        0
    );
    sessions.cancel(99); // unknown: nothing happens

    let b = sessions.open(fs.clone(), &path).await.unwrap();
    let c = sessions.open(fs.clone(), &path).await.unwrap();
    assert_eq!(sessions.open_count(), 2);
    assert!(
        sessions.read(a.session, 0, 1).await.is_err(),
        "oldest closed"
    );
    sessions.close(b.session);
    assert!(sessions.read(b.session, 0, 1).await.is_err());
    assert!(sessions.read(c.session, 0, 1).await.is_ok());

    let dir = std::path::Path::new(&path).parent().unwrap();
    assert!(sessions
        .open(fs.clone(), &dir.to_string_lossy())
        .await
        .is_err());
    assert!(matches!(
        sessions.open(fs, &format!("{path}.missing")).await,
        Err(Error::NotFound(_))
    ));
}

/// VIEW-011 — a UTF-16 text search ignores case on whole code units only
/// (U+0141 "Ł" is not U+0161 "š" without case) and finds matches on code
/// units only, forwards and backwards, seekable or not; a raw byte search
/// still matches anywhere.
#[tokio::test]
async fn utf16_search_folds_code_units_on_unit_boundaries() {
    let cancel = CancellationToken::new();
    // Each text has the bytes of "ab" across code units at offset 7.
    for (enc, text) in [
        (Encoding::Utf16Le, "xŁx\u{6100}\u{6200}\u{0100}aBš"),
        (Encoding::Utf16Be, "xŁx\u{0100}\u{6100}\u{6201}aBš"),
    ] {
        let data = encode(text, enc).unwrap();
        assert_eq!(&data[7..11], &encode("ab", enc).unwrap()[..]);
        for seekable in [true, false] {
            let (fs, path) = mem_file(&data, seekable);
            let size = data.len() as u64;
            let find = |needle: &str, from: u64, backward: bool, ignore_case: bool| {
                let needle = encode(needle, enc).unwrap();
                let (fs, path, cancel) = (fs.clone(), path.clone(), cancel.clone());
                async move {
                    let o = SearchOptions {
                        backward,
                        ignore_case,
                        encoding: Some(enc),
                        chunk: 3,
                    };
                    search(fs.as_ref(), &path, size, &needle, from, o, &cancel, |_| {})
                        .await
                        .unwrap()
                }
            };
            // "š" is only the last character, never "Ł".
            assert_eq!(find("š", 0, false, true).await, Some(16), "{enc:?}");
            assert_eq!(find("š", size, true, true).await, Some(16), "{enc:?}");
            assert_eq!(find("Š", 0, false, true).await, None, "{enc:?}");
            assert_eq!(find("ł", 0, false, true).await, None, "{enc:?}");
            // "ab": the "aB" at 12, never the bytes across units at 7.
            assert_eq!(find("ab", 0, false, true).await, Some(12), "{enc:?}");
            assert_eq!(find("ab", size, true, true).await, Some(12), "{enc:?}");
            assert_eq!(find("ab", 0, false, false).await, None, "{enc:?}");
            assert_eq!(find("aB", 0, false, false).await, Some(12), "{enc:?}");
        }
    }
    // A raw byte search matches at any offset.
    let data = encode("\u{6100}\u{6200}", Encoding::Utf16Le).unwrap(); // 00 61 00 62
    let (fs, path) = mem_file(&data, true);
    let raw = search(
        fs.as_ref(),
        &path,
        4,
        &[0x61, 0x00],
        0,
        SearchOptions::default(),
        &cancel,
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(raw, Some(1));
}

/// VIEW-012 — a search or line count is cancelled by its request id while it
/// waits for a page read on the same file to finish.
#[tokio::test]
async fn sessions_cancel_while_waiting_for_the_reader() {
    let sessions = Arc::new(Sessions::new(2));
    let data = pseudo_random(300_000, 3);
    let (fs, path) = mem_file(&data, true);
    let f = sessions.open(fs.clone(), &path).await.unwrap();
    fs.latency(FaultOp::OpenReadAt, Duration::from_millis(400));
    // A page that is not cached holds the reader while it is fetched.
    let reading = {
        let s = sessions.clone();
        tokio::spawn(async move { s.read(f.session, 200_000, 10).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let searching = {
        let s = sessions.clone();
        tokio::spawn(async move {
            s.search(f.session, 7, b"zz", 0, SearchOptions::default(), |_| {})
                .await
        })
    };
    let counting = {
        let s = sessions.clone();
        tokio::spawn(async move { s.count_lines(f.session, 8, 300_000, Encoding::Utf8).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    sessions.cancel(7);
    sessions.cancel(8);
    assert!(matches!(searching.await.unwrap(), Err(Error::Cancelled)));
    assert!(matches!(counting.await.unwrap(), Err(Error::Cancelled)));
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "stopped without waiting for the read"
    );
    assert_eq!(reading.await.unwrap().unwrap(), &data[200_000..200_010]);
}

/// VIEW-010 — a 1 GiB local file opens and reads its end at once: only the
/// pages asked for are read.
#[tokio::test]
async fn one_gigabyte_file_stays_fast() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.bin");
    {
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(1 << 30).unwrap(); // sparse on most file systems
        use std::io::{Seek, SeekFrom, Write};
        let mut f = f;
        f.seek(SeekFrom::Start((1 << 30) - 6)).unwrap();
        f.write_all(b"THEEND").unwrap();
    }
    let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new());
    let p = path.to_string_lossy().into_owned();
    let sessions = Sessions::default();
    let started = Instant::now();
    let f = sessions.open(fs.clone(), &p).await.unwrap();
    assert_eq!(f.size, 1 << 30);
    let tail = sessions.read(f.session, (1 << 30) - 6, 100).await.unwrap();
    assert_eq!(tail, b"THEEND");
    let mid = sessions.read(f.session, 500_000_000, 4096).await.unwrap();
    assert_eq!(mid.len(), 4096);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    // Searching backwards from the end finds the marker in the last chunk.
    let found = sessions
        .search(
            f.session,
            1,
            b"theend",
            1 << 30,
            SearchOptions {
                backward: true,
                ignore_case: true,
                ..Default::default()
            },
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(found, Some((1 << 30) - 6));
}

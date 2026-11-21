//! The built-in file compare view's core (DIFF-010…021): line diffs and
//! their options, intra-line changes, binary files, the size limit, copying
//! changes and saving through the file system.

use std::sync::Arc;

use dualpal_core::diff::{
    diff_lines, intra_line, join_lines, split_lines, DiffDoc, DiffOptions, DiffOutcome,
    DiffSessions, Endings, Eol, Hunk, Line, Side, MAX_BYTES, MAX_LINES,
};
use dualpal_core::vfs::{FileSystem, LocalFs};
use dualpal_core::viewer::Encoding;
use dualpal_testkit::{FaultyFs, MemFs};

const NONE: DiffOptions = DiffOptions {
    ignore_whitespace: false,
    ignore_case: false,
    ignore_line_endings: false,
};

fn hunk(ls: u32, ll: u32, rs: u32, rl: u32) -> Hunk {
    Hunk {
        left_start: ls,
        left_len: ll,
        right_start: rs,
        right_len: rl,
    }
}

fn hunks(a: &str, b: &str, o: DiffOptions) -> Vec<Hunk> {
    diff_lines(&split_lines(a), &split_lines(b), &o)
}

/// DIFF-010 — identical files have no changes; splitting and joining keep
/// every byte.
#[test]
fn identical_files_have_no_changes() {
    let text = "one\ntwo\r\nthree";
    assert!(hunks(text, text, NONE).is_empty());
    let doc = DiffDoc::from_texts(text, text, NONE);
    assert!(doc.identical());
    assert_eq!(
        split_lines(text),
        vec![
            Line::new("one", Eol::Lf),
            Line::new("two", Eol::CrLf),
            Line::new("three", Eol::None),
        ]
    );
    assert_eq!(join_lines(&split_lines(text)), text);
}

/// DIFF-011 — inserted, deleted and changed lines become hunks with the
/// right line numbers on both sides.
#[test]
fn insert_delete_change() {
    let base = "a\nb\nc\nd\ne\n";
    // Inserted after b.
    assert_eq!(
        hunks(base, "a\nb\nX\nc\nd\ne\n", NONE),
        vec![hunk(2, 0, 2, 1)]
    );
    // d deleted.
    assert_eq!(hunks(base, "a\nb\nc\ne\n", NONE), vec![hunk(3, 1, 3, 0)]);
    // b changed, e changed: two hunks.
    assert_eq!(
        hunks(base, "a\nB\nc\nd\nE\n", NONE),
        vec![hunk(1, 1, 1, 1), hunk(4, 1, 4, 1)]
    );
    // Neighbouring delete and insert are one hunk.
    assert_eq!(
        hunks(base, "a\nx\ny\nz\nd\ne\n", NONE),
        vec![hunk(1, 2, 1, 3)]
    );
}

/// DIFF-012 — ignore whitespace and ignore case.
#[test]
fn whitespace_and_case_options() {
    let a = "fn main() {\n    let x = 1;\n}\n";
    let b = "fn  main() {\n\tlet x=1;\n}\n";
    assert_eq!(hunks(a, b, NONE).len(), 1);
    let ws = DiffOptions {
        ignore_whitespace: true,
        ..NONE
    };
    assert!(hunks(a, b, ws).is_empty());

    let c = "Hello World\nsame\n";
    let d = "hello world\nsame\n";
    assert_eq!(hunks(c, d, NONE), vec![hunk(0, 1, 0, 1)]);
    let case = DiffOptions {
        ignore_case: true,
        ..NONE
    };
    assert!(hunks(c, d, case).is_empty());
    // A doc's options can change without reading again.
    let mut doc = DiffDoc::from_texts(c, d, NONE);
    assert_eq!(doc.hunks.len(), 1);
    doc.set_options(case);
    assert!(doc.hunks.is_empty());
    assert!(!doc.identical());
}

/// DIFF-013 — CRLF and LF lines differ unless line endings are ignored,
/// which also ignores a missing line feed at the end.
#[test]
fn crlf_and_lf() {
    let lf = "a\nb\nc\n";
    let crlf = "a\r\nb\r\nc\r\n";
    assert_eq!(hunks(lf, crlf, NONE), vec![hunk(0, 3, 0, 3)]);
    let eol = DiffOptions {
        ignore_line_endings: true,
        ..NONE
    };
    assert!(hunks(lf, crlf, eol).is_empty());
    assert_eq!(hunks("a\nb\n", "a\nb", NONE), vec![hunk(1, 1, 1, 1)]);
    assert!(hunks("a\nb\n", "a\nb", eol).is_empty());
    let doc = DiffDoc::from_texts(lf, crlf, NONE);
    assert_eq!(doc.side(Side::Left).endings(), Endings::Lf);
    assert_eq!(doc.side(Side::Right).endings(), Endings::CrLf);
}

/// DIFF-014 — empty files: equal to each other, all added against text.
#[test]
fn empty_files() {
    assert!(split_lines("").is_empty());
    assert!(hunks("", "", NONE).is_empty());
    assert!(DiffDoc::from_texts("", "", NONE).identical());
    assert_eq!(hunks("", "a\nb\n", NONE), vec![hunk(0, 0, 0, 2)]);
    assert_eq!(hunks("a\n", "", NONE), vec![hunk(0, 1, 0, 0)]);
}

/// DIFF-017 — intra-line changes are whole words, in UTF-16 units, and
/// follow the options.
#[test]
fn intra_line_marks() {
    let (l, r) = intra_line("let total = 10;", "let sum = 10;", &NONE);
    assert_eq!(l, vec![[4, 9]]);
    assert_eq!(r, vec![[4, 7]]);
    // Characters outside the BMP count two units, as in JavaScript.
    let (l, r) = intra_line("😀 a b", "😀 a c", &NONE);
    assert_eq!(l, vec![[5, 6]]);
    assert_eq!(r, vec![[5, 6]]);
    // Only an insertion: nothing on the left.
    let (l, r) = intra_line("a b", "a x b", &NONE);
    assert!(l.is_empty());
    assert_eq!(r.len(), 1);
    let [a, b] = r[0];
    assert_eq!("a x b"[a as usize..b as usize].trim(), "x");
    let (l, r) = intra_line(
        "Foo  bar",
        "foo bar",
        &DiffOptions {
            ignore_case: true,
            ignore_whitespace: true,
            ..NONE
        },
    );
    assert!(l.is_empty() && r.is_empty());

    // A doc gives each changed line pair its marks; added lines have none.
    let doc = DiffDoc::from_texts("same\nold word\n", "same\nnew word\nadded\n", NONE);
    assert_eq!(doc.hunks, vec![hunk(1, 1, 1, 2)]);
    let left = doc.lines(Side::Left, 0, 10);
    assert_eq!(left.len(), 2);
    assert!(left[0].marks.is_empty());
    assert_eq!(left[1].marks, vec![[0, 3]]);
    let right = doc.lines(Side::Right, 1, 10);
    assert_eq!(right[0].text, "new word");
    assert_eq!(right[0].marks, vec![[0, 3]]);
    assert_eq!(right[1].text, "added");
    assert!(right[1].marks.is_empty());
    assert!(doc.lines(Side::Right, 99, 10).is_empty());
}

/// DIFF-018 — copying a change to either side makes that part equal and
/// marks the side as changed; stale revisions are refused.
#[test]
fn apply_hunk_left_and_right() {
    let mut doc = DiffDoc::from_texts("a\nb\nc\nd\n", "a\nB\nc\nd\nadded\n", NONE);
    assert_eq!(doc.hunks, vec![hunk(1, 1, 1, 1), hunk(4, 0, 4, 1)]);
    let rev = doc.revision;
    // b → right.
    doc.apply(rev, 0, Side::Right).unwrap();
    assert_eq!(
        join_lines(&doc.side(Side::Right).lines),
        "a\nb\nc\nd\nadded\n"
    );
    assert!(doc.side(Side::Right).dirty);
    assert!(!doc.side(Side::Left).dirty);
    assert_eq!(doc.hunks, vec![hunk(4, 0, 4, 1)]);
    assert!(doc.apply(rev, 0, Side::Left).is_err(), "stale revision");
    // "added" → left.
    doc.apply(doc.revision, 0, Side::Left).unwrap();
    assert_eq!(
        join_lines(&doc.side(Side::Left).lines),
        "a\nb\nc\nd\nadded\n"
    );
    assert!(doc.hunks.is_empty());
    assert!(doc.identical());
    assert!(
        doc.apply(doc.revision, 0, Side::Left).is_err(),
        "no such change"
    );

    // Removing lines: copying an empty side deletes them.
    let mut doc = DiffDoc::from_texts("a\nx\ny\nb\n", "a\nb\n", NONE);
    doc.apply(doc.revision, 0, Side::Left).unwrap();
    assert_eq!(join_lines(&doc.side(Side::Left).lines), "a\nb\n");

    // With line endings ignored, copied lines take the file's own, and a
    // last line without a line feed gets one when lines follow it.
    let eol = DiffOptions {
        ignore_line_endings: true,
        ..NONE
    };
    let mut doc = DiffDoc::from_texts("a\r\nb\r\nc", "a\nX\nc\nd\n", eol);
    assert_eq!(doc.hunks, vec![hunk(1, 1, 1, 1), hunk(3, 0, 3, 1)]);
    doc.apply(doc.revision, 1, Side::Left).unwrap();
    doc.apply(doc.revision, 0, Side::Left).unwrap();
    assert_eq!(
        join_lines(&doc.side(Side::Left).lines),
        "a\r\nX\r\nc\r\nd\r\n"
    );
}

fn mem(files: &[(&str, &[u8])]) -> Arc<FaultyFs<MemFs>> {
    let m = MemFs::with_default_id();
    for (p, c) in files {
        m.seed_file(p, c);
    }
    Arc::new(FaultyFs::new(m))
}

fn at(fs: &Arc<FaultyFs<MemFs>>, path: &str) -> (Arc<dyn FileSystem>, String) {
    (fs.clone() as Arc<dyn FileSystem>, path.to_string())
}

/// DIFF-015 — files that are not text are compared byte by byte, also on a
/// file system that cannot seek.
#[tokio::test]
async fn binary_files() {
    let mut a = vec![0u8; 300_000];
    a[0] = 0x7f;
    let mut b = a.clone();
    b[299_999] = 1;
    let fs = mem(&[
        ("/a.bin", &a),
        ("/a2.bin", &a),
        ("/b.bin", &b),
        ("/t.txt", b"text\n"),
    ]);
    fs.set_seekable(false);
    let s = DiffSessions::default();
    let open = |l: &str, r: &str| s.open(1, at(&fs, l), at(&fs, r), NONE, [None, None]);
    assert_eq!(
        open("/a.bin", "/a2.bin").await.unwrap(),
        DiffOutcome::Binary {
            identical: true,
            sizes: [300_000, 300_000]
        }
    );
    assert_eq!(
        open("/a.bin", "/b.bin").await.unwrap(),
        DiffOutcome::Binary {
            identical: false,
            sizes: [300_000, 300_000]
        }
    );
    assert!(matches!(
        open("/t.txt", "/b.bin").await.unwrap(),
        DiffOutcome::Binary {
            identical: false,
            ..
        }
    ));
    assert_eq!(s.open_count(), 0);
}

/// DIFF-016 — files over the size or line limit are refused with the limits.
#[tokio::test]
async fn size_limit() {
    let big = vec![b'x'; MAX_BYTES as usize + 1];
    let many = "\n".repeat(MAX_LINES + 1);
    let fs = mem(&[
        ("/big.txt", &big),
        ("/many.txt", many.as_bytes()),
        ("/s.txt", b"s\n"),
    ]);
    let s = DiffSessions::default();
    let out = s
        .open(
            1,
            at(&fs, "/big.txt"),
            at(&fs, "/s.txt"),
            NONE,
            [None, None],
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        DiffOutcome::TooLarge {
            sizes: [MAX_BYTES + 1, 2],
            max_bytes: MAX_BYTES,
            max_lines: MAX_LINES as u64
        }
    );
    let out = s
        .open(
            2,
            at(&fs, "/s.txt"),
            at(&fs, "/many.txt"),
            NONE,
            [None, None],
        )
        .await
        .unwrap();
    assert!(matches!(out, DiffOutcome::TooLarge { .. }));
    // Serialized with a kind tag for the UI.
    let json = serde_json::to_value(&out).unwrap();
    assert_eq!(json["kind"], "tooLarge");
    assert_eq!(json["maxLines"], MAX_LINES as u64);
}

fn text_state(out: DiffOutcome) -> dualpal_core::diff::DiffState {
    match out {
        DiffOutcome::Text { state } => state,
        other => panic!("expected text, got {other:?}"),
    }
}

/// DIFF-019 — a compare of a local file and a server file: lines, copying
/// a change and saving each side through its file system.
#[tokio::test]
async fn sessions_apply_and_save() {
    let tmp = tempfile::tempdir().unwrap();
    let local_path = tmp.path().join("left.txt");
    std::fs::write(&local_path, "one\ntwo\nthree\n").unwrap();
    let local: Arc<dyn FileSystem> = Arc::new(LocalFs::new());
    let server = mem(&[("/right.txt", b"one\n2\nthree\nfour\n")]);
    let s = DiffSessions::default();
    let state = text_state(
        s.open(
            7,
            (local.clone(), local_path.to_string_lossy().into_owned()),
            at(&server, "/right.txt"),
            NONE,
            [None, None],
        )
        .await
        .unwrap(),
    );
    assert_eq!(state.hunks, vec![hunk(1, 1, 1, 1), hunk(3, 0, 3, 1)]);
    assert_eq!(state.sides[0].lines, 3);
    assert_eq!(state.sides[1].lines, 4);
    assert_eq!(state.sides[0].encoding, Encoding::Utf8);
    assert!(state.sides[0].saveable && !state.sides[0].dirty);
    let session = state.session;

    let right = s.lines(session, Side::Right, 0, 100).await.unwrap();
    assert_eq!(
        right.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        ["one", "2", "three", "four"]
    );
    assert_eq!(right[1].marks, vec![[0, 1]]);

    // "two" → right, "four" → left.
    let state = s
        .apply(session, state.revision, 0, Side::Right)
        .await
        .unwrap();
    assert!(state.sides[1].dirty);
    let state = s
        .apply(session, state.revision, 0, Side::Left)
        .await
        .unwrap();
    assert!(state.hunks.is_empty() && state.identical);
    assert!(s
        .apply(session, state.revision - 1, 0, Side::Left)
        .await
        .is_err());

    let state = s.save(session, Side::Right).await.unwrap();
    assert!(!state.sides[1].dirty && state.sides[0].dirty);
    assert_eq!(
        server.inner().contents("/right.txt").unwrap(),
        b"one\ntwo\nthree\nfour\n"
    );
    let state = s.save(session, Side::Left).await.unwrap();
    assert!(!state.sides[0].dirty);
    assert_eq!(
        std::fs::read_to_string(&local_path).unwrap(),
        "one\ntwo\nthree\nfour\n"
    );
    // No temporary file is left behind.
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);

    // A file changed by someone else since it was read is not overwritten.
    std::fs::write(&local_path, "someone else\n").unwrap();
    let state = s.state(session).await.unwrap();
    assert_eq!(state.sides[0].size, 19);
    let err = s.save(session, Side::Left).await.unwrap_err();
    assert!(err.to_string().contains("changed since"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&local_path).unwrap(),
        "someone else\n"
    );

    s.close(session);
    assert!(s.lines(session, Side::Left, 0, 1).await.is_err());
}

/// DIFF-020 — encodings: UTF-16 with a BOM compares with UTF-8 and saves
/// back as UTF-16; a side can be read as another encoding; characters a
/// single-byte encoding lacks stop the save.
#[tokio::test]
async fn encodings_per_side() {
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("héllo\nworld\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let fs = mem(&[
        ("/u16.txt", &utf16),
        ("/u8.txt", "héllo\nWorld €\n".as_bytes()),
        ("/latin.txt", b"h\xe9llo\nworld\n"),
    ]);
    let s = DiffSessions::default();
    let state = text_state(
        s.open(
            1,
            at(&fs, "/u16.txt"),
            at(&fs, "/u8.txt"),
            NONE,
            [None, None],
        )
        .await
        .unwrap(),
    );
    assert_eq!(state.sides[0].encoding, Encoding::Utf16Le);
    assert!(state.sides[0].bom);
    assert_eq!(state.hunks, vec![hunk(1, 1, 1, 1)]);
    let state = s
        .apply(state.session, state.revision, 0, Side::Left)
        .await
        .unwrap();
    s.save(state.session, Side::Left).await.unwrap();
    let expect: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("héllo\nWorld €\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(fs.inner().contents("/u16.txt").unwrap(), expect);

    // Latin-1 text is detected as Windows-1252; "€" exists there, but a
    // character it lacks cannot be saved.
    let fs2 = mem(&[
        ("/latin.txt", b"h\xe9llo\nworld\n"),
        ("/u8.txt", "héllo\nwörld ✓\n".as_bytes()),
    ]);
    let state = text_state(
        s.open(
            2,
            at(&fs2, "/latin.txt"),
            at(&fs2, "/u8.txt"),
            NONE,
            [None, None],
        )
        .await
        .unwrap(),
    );
    assert_eq!(state.sides[0].encoding, Encoding::Windows1252);
    assert_eq!(state.hunks, vec![hunk(1, 1, 1, 1)]);
    let state = s
        .apply(state.session, state.revision, 0, Side::Left)
        .await
        .unwrap();
    let err = s.save(state.session, Side::Left).await.unwrap_err();
    assert!(err.to_string().contains("cannot be written"), "{err}");

    // Reading a side as another encoding changes its lines; refused with changes.
    assert!(s
        .set_encoding(state.session, Side::Left, Encoding::Utf8)
        .await
        .is_err());
    let state = s
        .set_encoding(state.session, Side::Right, Encoding::Latin1)
        .await
        .unwrap();
    assert_eq!(state.sides[1].encoding, Encoding::Latin1);
    let right = s.lines(state.session, Side::Right, 0, 1).await.unwrap();
    assert_eq!(right[0].text, "hÃ©llo");
    let state = s
        .set_options(
            state.session,
            DiffOptions {
                ignore_case: true,
                ..NONE
            },
        )
        .await
        .unwrap();
    assert!(state.options.ignore_case);
}

/// DIFF-021 — invalid UTF-8 falls back to a single-byte encoding, so the
/// bytes read back the same; a side that would not is not saved.
#[tokio::test]
async fn lossy_sides_are_not_saved() {
    let fs = mem(&[("/a.txt", b"a\xff\nb\n"), ("/b.txt", b"a\nb\n")]);
    let s = DiffSessions::default();
    let state = text_state(
        s.open(
            1,
            at(&fs, "/a.txt"),
            at(&fs, "/b.txt"),
            NONE,
            [Some(Encoding::Utf8), None],
        )
        .await
        .unwrap(),
    );
    assert!(
        !state.sides[0].saveable,
        "U+FFFD would replace the 0xFF byte"
    );
    let state = s
        .apply(state.session, state.revision, 0, Side::Left)
        .await
        .unwrap();
    assert!(s.save(state.session, Side::Left).await.is_err());
    assert_eq!(fs.inner().contents("/a.txt").unwrap(), b"a\xff\nb\n");
    let state = text_state(
        s.open(2, at(&fs, "/a.txt"), at(&fs, "/b.txt"), NONE, [None, None])
            .await
            .unwrap(),
    );
    assert_eq!(state.sides[0].encoding, Encoding::Windows1252);
    assert!(state.sides[0].saveable);
}

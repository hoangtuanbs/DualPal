//! Text reading/writing cases (Phase 1, §8.2 VFS-TXT).

use dualpal_core::vfs::{read_text, write_text, LocalFs, TextFile};

/// VFS-TXT-002 — a NUL byte marks the file as binary.
#[tokio::test]
async fn binary_detection() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x.bin");
    std::fs::write(&p, [0x41u8, 0, 0x42]).unwrap();
    let fs = LocalFs::new();
    let t = read_text(&fs, &p.to_string_lossy(), 100).await.unwrap();
    assert!(t.binary);
    assert!(t.text.is_empty(), "binary content is not shown as text");
}

/// VFS-TXT-003 — CRLF is preserved byte-for-byte through read + write.
#[tokio::test]
async fn crlf_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("crlf.txt");
    std::fs::write(&p, b"a\r\nb\r\n").unwrap();
    let fs = LocalFs::new();
    let path = p.to_string_lossy();
    let t = read_text(&fs, &path, 100).await.unwrap();
    assert_eq!(t.text, "a\r\nb\r\n");
    write_text(&fs, &path, &t.text).await.unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nb\r\n");
}

/// VFS-TXT-003 — a UTF-16 BOM is not valid text, so it reads as binary.
#[tokio::test]
async fn utf16_bom_reads_as_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("u16.txt");
    std::fs::write(&p, [0xFFu8, 0xFE, b'h', 0, b'i', 0]).unwrap();
    let fs = LocalFs::new();
    let t = read_text(&fs, &p.to_string_lossy(), 100).await.unwrap();
    assert!(t.binary);
}

/// VFS-TXT-004 — truncation never splits a UTF-8 character.
#[tokio::test]
async fn truncation_never_splits_a_character() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("cjk.txt");
    std::fs::write(&p, "中文文件").unwrap(); // 4 chars × 3 bytes
    let fs = LocalFs::new();
    let t = read_text(&fs, &p.to_string_lossy(), 4).await.unwrap();
    assert!(t.truncated);
    assert!(!t.binary);
    // 4 bytes = one full char (3) + one byte of the next; never a broken char.
    assert_eq!(t.text, "中");
    assert!(t.text.is_char_boundary(t.text.len()));
}

// Keep the `TextFile` type referenced so the shape is part of the public
// contract the UI depends on.
#[test]
fn text_file_shape() {
    let t = TextFile {
        text: "hi".into(),
        truncated: false,
        binary: false,
    };
    insta::assert_json_snapshot!(t, @r###"
    {
      "text": "hi",
      "truncated": false,
      "binary": false
    }
    "###);
}

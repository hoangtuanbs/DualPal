//! What a file or folder name may be on the local disk.
//!
//! Every name the app writes to this computer (a copy or move target, a
//! download, a new folder, a rename, a file opened for editing) is checked
//! here, so one set of rules applies everywhere and a bad name gets one clear
//! error instead of whatever the operating system happens to report.

/// The naming rules of a family of file systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRules {
    /// NTFS, FAT and exFAT as Windows sees them.
    Windows,
    /// Linux and macOS disks: only `/` and NUL are refused, up to 255 bytes.
    Unix,
}

impl NameRules {
    /// The rules of the disk this app runs on.
    pub const LOCAL: NameRules = if cfg!(windows) {
        NameRules::Windows
    } else {
        NameRules::Unix
    };
}

/// Longest name, in UTF-16 units on Windows and in bytes elsewhere.
pub const MAX_NAME: usize = 255;

/// Device names Windows keeps for itself, in any case and with any extension
/// ("con", "NUL.txt", "com1.tar.gz"). `¹²³` are the superscript digits
/// Windows also matches after COM and LPT.
const WINDOWS_RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

fn windows_reserved(name: &str) -> Option<String> {
    // Windows ignores everything from the first dot, and spaces before it.
    let base = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = base.to_uppercase();
    if WINDOWS_RESERVED.contains(&upper.as_str()) {
        return Some(upper);
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: String = chars.collect();
    let digit = matches!(
        rest.as_str(),
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    ) || matches!(rest.as_str(), "¹" | "²" | "³");
    ((prefix == "COM" || prefix == "LPT") && digit).then_some(upper)
}

fn windows_bad_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32
}

/// Why `name` cannot be used as a file or folder name under `rules`, if it
/// cannot. The reason is a sentence for the user.
pub fn name_problem(name: &str, rules: NameRules) -> Option<String> {
    if name.is_empty() {
        return Some("a name can't be empty".into());
    }
    if name == "." || name == ".." {
        return Some(format!("\"{name}\" can't be used as a name"));
    }
    match rules {
        NameRules::Unix => {
            if name.contains(['/', '\0']) {
                return Some("a name can't contain / or a NUL character".into());
            }
            if name.len() > MAX_NAME {
                return Some(format!("the name is longer than {MAX_NAME} bytes"));
            }
        }
        NameRules::Windows => {
            if let Some(c) = name.chars().find(|&c| windows_bad_char(c)) {
                let shown = if (c as u32) < 32 {
                    "control characters".to_string()
                } else {
                    c.to_string()
                };
                return Some(format!(
                    "Windows doesn't allow {shown} in a name (none of < > : \" / \\ | ? * or control characters)"
                ));
            }
            if name.ends_with('.') || name.ends_with(' ') {
                return Some("Windows doesn't allow a name to end with a dot or a space".into());
            }
            if let Some(device) = windows_reserved(name) {
                return Some(format!(
                    "{device} is a device name that Windows keeps for itself, with or without an extension"
                ));
            }
            if name.encode_utf16().count() > MAX_NAME {
                return Some(format!("the name is longer than {MAX_NAME} characters"));
            }
        }
    }
    None
}

/// Why `name` cannot be written to this computer's disk, if it cannot.
pub fn local_name_problem(name: &str) -> Option<String> {
    name_problem(name, NameRules::LOCAL)
}

/// `name` changed as little as possible so that [`name_problem`] accepts it
/// under `rules`: refused characters become `_`, a reserved device name or a
/// trailing dot or space gets `_` added, and a long name is shortened, keeping
/// its extension.
pub fn sanitize(name: &str, rules: NameRules) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            let bad = match rules {
                NameRules::Unix => c == '/' || c == '\0',
                NameRules::Windows => windows_bad_char(c),
            };
            if bad {
                '_'
            } else {
                c
            }
        })
        .collect();
    if out.is_empty() || out == "." || out == ".." {
        return "file".into();
    }
    if rules == NameRules::Windows {
        if out.ends_with('.') || out.ends_with(' ') {
            out.push('_');
        }
        if windows_reserved(&out).is_some() {
            // "con.txt" becomes "con_.txt", "nul" becomes "nul_".
            match out.find('.') {
                Some(dot) => out.insert(dot, '_'),
                None => out.push('_'),
            }
        }
    }
    shorten(out, rules)
}

fn units(s: &str, rules: NameRules) -> usize {
    match rules {
        NameRules::Unix => s.len(),
        NameRules::Windows => s.encode_utf16().count(),
    }
}

/// Cuts `name` to [`MAX_NAME`], keeping a short extension.
fn shorten(name: String, rules: NameRules) -> String {
    if units(&name, rules) <= MAX_NAME {
        return name;
    }
    let ext = match name.rfind('.') {
        Some(dot) if dot > 0 && units(&name[dot..], rules) <= 16 => &name[dot..],
        _ => "",
    };
    let room = MAX_NAME - units(ext, rules);
    let mut stem = String::new();
    for c in name[..name.len() - ext.len()].chars() {
        if units(&stem, rules) + units(c.encode_utf8(&mut [0; 4]), rules) > room {
            break;
        }
        stem.push(c);
    }
    // Cutting can leave a trailing dot or space, which Windows refuses.
    if rules == NameRules::Windows {
        while stem.ends_with('.') || stem.ends_with(' ') {
            stem.pop();
        }
        if stem.is_empty() {
            stem.push('_');
        }
    }
    stem + ext
}

#[cfg(test)]
mod tests {
    use super::*;
    use NameRules::{Unix, Windows};

    #[test]
    fn windows_reserved_names_in_any_case_and_with_extensions() {
        for name in [
            "con",
            "CON",
            "Con.txt",
            "nul",
            "NUL.tar.gz",
            "prn",
            "aux.",
            "com1",
            "COM9.log",
            "lpt1",
            "Lpt5.txt",
            "com0",
            "COM¹",
            "lpt³.x",
            "conin$",
            "CONOUT$.txt",
            "con .txt",
        ] {
            assert!(name_problem(name, Windows).is_some(), "{name:?}");
        }
        for name in [
            "console",
            "con_",
            "nul1",
            "com10",
            "lpt",
            "xcon",
            "a.con",
            "com",
            "auxiliary.txt",
        ] {
            assert_eq!(name_problem(name, Windows), None, "{name:?}");
        }
    }

    #[test]
    fn windows_characters_and_endings() {
        for name in [
            "a<b", "a>b", "a:b", "a\"b", "a/b", "a\\b", "a|b", "a?b", "a*b", "a\tb", "a\u{1}",
        ] {
            assert!(name_problem(name, Windows).is_some(), "{name:?}");
        }
        for name in ["a.", "a ", "a. ", "...", " "] {
            assert!(name_problem(name, Windows).is_some(), "{name:?}");
        }
        for name in [" a", ".hidden", "a b.txt", "ünïcödé.txt", "日本語"] {
            assert_eq!(name_problem(name, Windows), None, "{name:?}");
        }
    }

    #[test]
    fn unix_allows_everything_but_slash_and_nul() {
        for name in ["con", "a:b", "a?*<>|\"", "a.", "a ", "a\\b"] {
            assert_eq!(name_problem(name, Unix), None, "{name:?}");
        }
        for name in ["", ".", "..", "a/b", "a\0b"] {
            assert!(name_problem(name, Unix).is_some(), "{name:?}");
        }
    }

    #[test]
    fn length_limits() {
        let ok = "a".repeat(255);
        assert_eq!(name_problem(&ok, Unix), None);
        assert_eq!(name_problem(&ok, Windows), None);
        assert!(name_problem(&"a".repeat(256), Unix).is_some());
        assert!(name_problem(&"a".repeat(256), Windows).is_some());
        // 100 × 3 bytes: too long in bytes, fine in UTF-16 units.
        let wide = "€".repeat(100);
        assert!(name_problem(&wide, Unix).is_some());
        assert_eq!(name_problem(&wide, Windows), None);
    }

    #[test]
    fn sanitize_changes_little() {
        assert_eq!(sanitize("con", Windows), "con_");
        assert_eq!(sanitize("CON.txt", Windows), "CON_.txt");
        assert_eq!(sanitize("a:b?.txt", Windows), "a_b_.txt");
        assert_eq!(sanitize("a.", Windows), "a._");
        assert_eq!(sanitize("..", Windows), "file");
        assert_eq!(sanitize("con", Unix), "con");
        assert_eq!(sanitize("a/b", Unix), "a_b");
        assert_eq!(sanitize("ok.txt", Windows), "ok.txt");
        let long = format!("{}.txt", "x".repeat(300));
        let short = sanitize(&long, Windows);
        assert_eq!(short.len(), 255);
        assert!(short.ends_with(".txt"));
        let wide = format!("{}.txt", "€".repeat(100));
        let short = sanitize(&wide, Unix);
        assert!(short.len() <= 255 && short.ends_with(".txt"), "{short}");
    }

    #[test]
    fn sanitized_names_always_pass() {
        let samples = [
            "",
            ".",
            "..",
            "con",
            "Con.txt",
            "a.",
            "a ",
            " . ",
            "a:b",
            "lpt1. ",
            "nul .x",
            "x/../y",
            "\u{1}\u{2}",
            "com¹",
            "a....",
        ];
        for rules in [Windows, Unix] {
            for s in samples {
                let out = sanitize(s, rules);
                assert_eq!(
                    name_problem(&out, rules),
                    None,
                    "{rules:?} {s:?} -> {out:?}"
                );
            }
            let long = format!("{}. .txt", "y".repeat(300));
            let out = sanitize(&long, rules);
            assert_eq!(name_problem(&out, rules), None, "{rules:?} long -> {out:?}");
        }
    }
}

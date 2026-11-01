//! External file compare apps (FileMerge, Beyond Compare, WinMerge, …).
//!
//! A tool is a command line template where `{left}` and `{right}` stand for
//! the two files, e.g. `"/usr/bin/opendiff" {left} {right}`. Templates are
//! split like a shell would (double or single quotes group words), but no
//! shell is involved, so file names are never interpreted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffTool {
    pub name: String,
    /// Command line template with `{left}` and `{right}`.
    pub command: String,
    /// Can compare two folders as well as two files.
    pub folders: bool,
}

struct Known {
    name: &'static str,
    /// Candidate programs: absolute paths, or names looked up on PATH.
    programs: &'static [&'static str],
    args: &'static str,
    folders: bool,
}

#[cfg(target_os = "macos")]
const KNOWN: &[Known] = &[
    Known {
        name: "FileMerge",
        programs: &["/Applications/Xcode.app/Contents/Developer/usr/bin/opendiff"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Beyond Compare",
        programs: &[
            "/Applications/Beyond Compare.app/Contents/MacOS/bcomp",
            "bcomp",
        ],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Kaleidoscope",
        programs: &[
            "/usr/local/bin/ksdiff",
            "/opt/homebrew/bin/ksdiff",
            "ksdiff",
        ],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Visual Studio Code",
        programs: &[
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
            "code",
        ],
        args: "--diff {left} {right}",
        folders: false,
    },
    Known {
        name: "BBEdit",
        programs: &[
            "/usr/local/bin/bbdiff",
            "/opt/homebrew/bin/bbdiff",
            "bbdiff",
        ],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Araxis Merge",
        programs: &["/Applications/Araxis Merge.app/Contents/Utilities/compare"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "P4Merge",
        programs: &["/Applications/p4merge.app/Contents/Resources/launchp4merge"],
        args: "{left} {right}",
        folders: false,
    },
    Known {
        name: "DeltaWalker",
        programs: &["/Applications/DeltaWalker.app/Contents/MacOS/DeltaWalker"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Meld",
        programs: &["/Applications/Meld.app/Contents/MacOS/Meld", "meld"],
        args: "{left} {right}",
        folders: true,
    },
];

#[cfg(windows)]
const KNOWN: &[Known] = &[
    Known {
        name: "WinMerge",
        programs: &[
            r"%ProgramFiles%\WinMerge\WinMergeU.exe",
            r"%LOCALAPPDATA%\Programs\WinMerge\WinMergeU.exe",
            r"%ProgramFiles(x86)%\WinMerge\WinMergeU.exe",
        ],
        args: "/e /u {left} {right}",
        folders: true,
    },
    Known {
        name: "Beyond Compare",
        programs: &[
            r"%ProgramFiles%\Beyond Compare 5\BComp.exe",
            r"%ProgramFiles%\Beyond Compare 4\BComp.exe",
        ],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Visual Studio Code",
        programs: &[
            r"%LOCALAPPDATA%\Programs\Microsoft VS Code\Code.exe",
            r"%ProgramFiles%\Microsoft VS Code\Code.exe",
        ],
        args: "--diff {left} {right}",
        folders: false,
    },
    Known {
        name: "Meld",
        programs: &[
            r"%ProgramFiles%\Meld\Meld.exe",
            r"%LOCALAPPDATA%\Programs\Meld\Meld.exe",
        ],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "TortoiseMerge",
        programs: &[
            r"%ProgramFiles%\TortoiseSVN\bin\TortoiseMerge.exe",
            r"%ProgramFiles%\TortoiseGit\bin\TortoiseGitMerge.exe",
        ],
        args: "/base:{left} /mine:{right}",
        folders: false,
    },
    Known {
        name: "Araxis Merge",
        programs: &[r"%ProgramFiles%\Araxis\Araxis Merge\Compare.exe"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "P4Merge",
        programs: &[r"%ProgramFiles%\Perforce\p4merge.exe"],
        args: "{left} {right}",
        folders: false,
    },
];

#[cfg(not(any(target_os = "macos", windows)))]
const KNOWN: &[Known] = &[
    Known {
        name: "Meld",
        programs: &["meld"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "KDiff3",
        programs: &["kdiff3"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Beyond Compare",
        programs: &["bcompare"],
        args: "{left} {right}",
        folders: true,
    },
    Known {
        name: "Visual Studio Code",
        programs: &["code"],
        args: "--diff {left} {right}",
        folders: false,
    },
];

/// Expand `%VAR%` (Windows) in a candidate path.
fn expand_env(p: &str) -> String {
    let mut out = String::new();
    let mut rest = p;
    while let Some(a) = rest.find('%') {
        let Some(len) = rest[a + 1..].find('%') else {
            break;
        };
        out.push_str(&rest[..a]);
        let var = &rest[a + 1..a + 1 + len];
        out.push_str(&std::env::var(var).unwrap_or_default());
        rest = &rest[a + len + 2..];
    }
    out.push_str(rest);
    out
}

/// The full path of `program`: itself if it is a path that exists,
/// otherwise its location on PATH.
pub fn find_program(program: &str) -> Option<PathBuf> {
    let p = PathBuf::from(expand_env(program));
    if p.components().count() > 1 || p.is_absolute() {
        return p.is_file().then_some(p);
    }
    let exts: Vec<String> = if cfg!(windows) {
        vec![String::new(), ".exe".into(), ".cmd".into(), ".bat".into()]
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| exts.iter().map(move |e| dir.join(format!("{program}{e}"))))
        .find(|c| c.is_file())
}

fn quote(p: &Path) -> String {
    format!("\"{}\"", p.display())
}

/// Compare tools installed on this computer, best known first.
pub fn detect() -> Vec<DiffTool> {
    KNOWN
        .iter()
        .filter_map(|k| {
            let path = k.programs.iter().find_map(|p| find_program(p))?;
            Some(DiffTool {
                name: k.name.into(),
                command: format!("{} {}", quote(&path), k.args),
                folders: k.folders,
            })
        })
        .collect()
}

/// Split a command line template into words and fill in the two paths.
/// When the template names neither `{left}` nor `{right}`, the paths are
/// added at the end.
pub fn command_line(template: &str, left: &str, right: &str) -> Result<Vec<String>> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in template.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::Invalid(
            "the compare command has an unclosed quote".into(),
        ));
    }
    if in_word {
        words.push(cur);
    }
    if words.is_empty() {
        return Err(Error::Invalid("the compare command is empty".into()));
    }
    let has_tokens = words
        .iter()
        .any(|w| w.contains("{left}") || w.contains("{right}"));
    let mut out: Vec<String> = words
        .into_iter()
        .map(|w| w.replace("{left}", left).replace("{right}", right))
        .collect();
    if !has_tokens {
        out.push(left.into());
        out.push(right.into());
    }
    Ok(out)
}

/// Start the tool on two local paths without waiting for it.
pub fn launch(template: &str, left: &str, right: &str) -> Result<()> {
    let argv = command_line(template, left, right)?;
    let program = find_program(&argv[0]).unwrap_or_else(|| PathBuf::from(&argv[0]));
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| {
        Error::Unsupported(format!(
            "starting the compare tool {} ({e})",
            program.display()
        ))
    })?;
    // Reap it when it exits so it does not linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_split_and_fill() {
        assert_eq!(
            command_line(
                r#""/Applications/My Tool.app/x" --diff {left} {right}"#,
                "/a b",
                "/c"
            )
            .unwrap(),
            vec!["/Applications/My Tool.app/x", "--diff", "/a b", "/c"]
        );
        assert_eq!(
            command_line("tool /base:{left} '/mine:{right}'", "L", "R").unwrap(),
            vec!["tool", "/base:L", "/mine:R"]
        );
        assert_eq!(
            command_line("meld", "L", "R").unwrap(),
            vec!["meld", "L", "R"]
        );
        assert!(command_line("\"open", "L", "R").is_err());
        assert!(command_line("  ", "L", "R").is_err());
    }

    #[test]
    fn env_vars_expand() {
        std::env::set_var("DP_TEST_DIR", "C:\\Tools");
        assert_eq!(expand_env(r"%DP_TEST_DIR%\x.exe"), r"C:\Tools\x.exe");
        assert_eq!(expand_env("100%"), "100%");
    }
}

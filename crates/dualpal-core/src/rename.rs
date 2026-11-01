//! Multi Rename: build new names for many items from a list of rules, and
//! keep named presets of rules.
//!
//! Rules apply in order to each name. A rule works on the name without its
//! extension, on the extension, or on the whole name ([`Scope`]). Folders
//! have no extension. Inserted text may hold tokens:
//! `{name}` the original name without extension, `{ext}` the original
//! extension, `{parent}` the folder name, `{date}` the modification date
//! (YYYY-MM-DD), `{n}` the item's number (1, 2, …) and `{n:3}` the number
//! padded to 3 digits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::vfs::Location;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// The name without its extension.
    #[default]
    Name,
    Extension,
    /// Name and extension together.
    Whole,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Position {
    #[default]
    Start,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaseKind {
    Lower,
    Upper,
    /// Every Word Capitalized.
    Title,
    /// First letter capitalized, the rest lower case.
    Sentence,
}

fn one() -> i64 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RenameRule {
    /// Replace text, or a regular expression (`$1` refers to groups).
    Replace {
        find: String,
        #[serde(default)]
        replace: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        scope: Scope,
    },
    Case {
        case: CaseKind,
        #[serde(default)]
        scope: Scope,
    },
    /// Insert text (with tokens) at the start or end.
    Insert {
        text: String,
        #[serde(default)]
        at: Position,
        #[serde(default)]
        scope: Scope,
    },
    /// Add a counter: `start`, `start + step`, … padded with zeros to `pad` digits.
    Number {
        #[serde(default = "one")]
        start: i64,
        #[serde(default = "one")]
        step: i64,
        #[serde(default)]
        pad: u32,
        #[serde(default)]
        at: Position,
        #[serde(default)]
        separator: String,
    },
    /// Remove `count` characters, counted from the start or the end.
    Remove {
        count: usize,
        #[serde(default)]
        from: Position,
        /// Characters to leave before the removed ones.
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        scope: Scope,
    },
    /// Set the extension (without the dot); empty removes it.
    Extension { ext: String },
}

/// What a rule needs to know about one item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameInput {
    pub name: String,
    #[serde(default)]
    pub dir: bool,
    #[serde(default)]
    pub modified_ms: Option<i64>,
    /// Name of the folder holding the item.
    #[serde(default)]
    pub parent: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreview {
    pub name: String,
    pub new_name: String,
    /// Why this name cannot be used, if it cannot.
    pub issue: Option<String>,
}

/// One item of a batch rename operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameItem {
    pub location: Location,
    pub new_name: String,
}

fn split(name: &str, dir: bool) -> (String, String) {
    if dir {
        return (name.to_string(), String::new());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i + 1..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

fn joined(stem: &str, ext: &str) -> String {
    if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    }
}

fn change_case(s: &str, case: CaseKind) -> String {
    match case {
        CaseKind::Lower => s.to_lowercase(),
        CaseKind::Upper => s.to_uppercase(),
        CaseKind::Title => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for c in s.chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start = !c.is_alphanumeric() && c != '\'';
            }
            out
        }
        CaseKind::Sentence => {
            let lower = s.to_lowercase();
            let mut chars = lower.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => lower,
            }
        }
    }
}

fn expand(text: &str, input: &RenameInput, index: usize) -> String {
    let (stem, ext) = split(&input.name, input.dir);
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        let value = match token {
            "name" => Some(stem.clone()),
            "ext" => Some(ext.clone()),
            "parent" => Some(input.parent.clone()),
            "date" => Some(date(input.modified_ms)),
            "n" => Some((index + 1).to_string()),
            t if t.starts_with("n:") => t[2..]
                .parse::<usize>()
                .ok()
                .map(|w| format!("{:0w$}", index + 1, w = w.min(12))),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn date(ms: Option<i64>) -> String {
    use chrono::TimeZone;
    ms.and_then(|ms| chrono::Local.timestamp_millis_opt(ms).earliest())
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Rules with their regular expressions compiled once.
pub struct Renamer {
    rules: Vec<(RenameRule, Option<regex::Regex>)>,
}

impl Renamer {
    pub fn new(rules: &[RenameRule]) -> Result<Self> {
        let mut out = Vec::new();
        for r in rules {
            let re = match r {
                RenameRule::Replace {
                    find,
                    regex,
                    case_sensitive,
                    ..
                } if !find.is_empty() => {
                    let pattern = if *regex {
                        find.clone()
                    } else {
                        regex::escape(find)
                    };
                    Some(
                        regex::RegexBuilder::new(&pattern)
                            .case_insensitive(!case_sensitive)
                            .build()
                            .map_err(|e| Error::Invalid(format!("pattern {find:?}: {e}")))?,
                    )
                }
                _ => None,
            };
            out.push((r.clone(), re));
        }
        Ok(Self { rules: out })
    }

    /// The new name of `input`, the `index`-th item of the batch.
    pub fn apply(&self, input: &RenameInput, index: usize) -> String {
        let (mut stem, mut ext) = split(&input.name, input.dir);
        for (rule, re) in &self.rules {
            // Run `f` on the part of the name the scope selects.
            let mut on = |scope: Scope, f: &dyn Fn(&str) -> String| match scope {
                Scope::Name => stem = f(&stem),
                Scope::Extension => {
                    if !input.dir {
                        ext = f(&ext)
                    }
                }
                Scope::Whole => {
                    let whole = f(&joined(&stem, &ext));
                    (stem, ext) = split(&whole, input.dir);
                }
            };
            match rule {
                RenameRule::Replace {
                    replace,
                    regex,
                    scope,
                    ..
                } => {
                    if let Some(re) = re {
                        let replace = if *regex {
                            replace.clone()
                        } else {
                            replace.replace('$', "$$")
                        };
                        on(*scope, &|s| {
                            re.replace_all(s, replace.as_str()).into_owned()
                        });
                    }
                }
                RenameRule::Case { case, scope } => on(*scope, &|s| change_case(s, *case)),
                RenameRule::Insert { text, at, scope } => {
                    let t = expand(text, input, index);
                    on(*scope, &|s| match at {
                        Position::Start => format!("{t}{s}"),
                        Position::End => format!("{s}{t}"),
                    });
                }
                RenameRule::Number {
                    start,
                    step,
                    pad,
                    at,
                    separator,
                } => {
                    let n = start.saturating_add(step.saturating_mul(index as i64));
                    let digits = if n < 0 {
                        format!("-{:0w$}", n.unsigned_abs(), w = (*pad).min(12) as usize)
                    } else {
                        format!("{:0w$}", n, w = (*pad).min(12) as usize)
                    };
                    on(Scope::Name, &|s| {
                        if s.is_empty() {
                            digits.clone()
                        } else {
                            match at {
                                Position::Start => format!("{digits}{separator}{s}"),
                                Position::End => format!("{s}{separator}{digits}"),
                            }
                        }
                    });
                }
                RenameRule::Remove {
                    count,
                    from,
                    offset,
                    scope,
                } => on(*scope, &|s| {
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len();
                    let (a, b) = match from {
                        Position::Start => {
                            let a = (*offset).min(len);
                            (a, (a + count).min(len))
                        }
                        Position::End => {
                            let b = len.saturating_sub(*offset);
                            (b.saturating_sub(*count), b)
                        }
                    };
                    chars[..a].iter().chain(&chars[b..]).collect()
                }),
                RenameRule::Extension { ext: new } => {
                    if !input.dir {
                        ext = new.trim_start_matches('.').to_string();
                    }
                }
            }
        }
        joined(&stem, &ext)
    }
}

/// Why `name` is not a usable file name, if it is not.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("empty name")
    } else if name == "." || name == ".." {
        Some("not a valid name")
    } else if name.contains(['/', '\\', '\0']) || name.chars().any(|c| c.is_control()) {
        Some("contains / or \\")
    } else if name.chars().count() > 255 {
        Some("name too long")
    } else {
        None
    }
}

/// New names for `inputs` (in order) and the problems with them.
/// `others` are names of items in the same folder that are not being
/// renamed; a new name may not take one of them.
pub fn preview(
    rules: &[RenameRule],
    inputs: &[RenameInput],
    others: &[String],
) -> Result<Vec<RenamePreview>> {
    let renamer = Renamer::new(rules)?;
    let new_names: Vec<String> = inputs
        .iter()
        .enumerate()
        .map(|(i, input)| renamer.apply(input, i))
        .collect();
    // Names compare without case: most Mac and Windows disks ignore it.
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in &new_names {
        *count.entry(n.to_lowercase()).or_default() += 1;
    }
    let taken: std::collections::HashSet<String> =
        others.iter().map(|n| n.to_lowercase()).collect();
    Ok(inputs
        .iter()
        .zip(new_names)
        .map(|(input, new_name)| {
            let key = new_name.to_lowercase();
            let issue = name_problem(&new_name)
                .map(str::to_string)
                .or_else(|| (count[&key] > 1).then(|| "same name as another item".to_string()))
                .or_else(|| {
                    taken
                        .contains(&key)
                        .then(|| "an item with this name already exists".to_string())
                });
            RenamePreview {
                name: input.name.clone(),
                new_name,
                issue,
            }
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenamePreset {
    pub name: String,
    pub rules: Vec<RenameRule>,
    /// Comes with the app and cannot be removed.
    #[serde(default)]
    pub builtin: bool,
}

/// Presets that come with the app.
pub fn builtin_presets() -> Vec<RenamePreset> {
    let p = |name: &str, rules: Vec<RenameRule>| RenamePreset {
        name: name.into(),
        rules,
        builtin: true,
    };
    vec![
        p(
            "Number in order (001, 002, …)",
            vec![RenameRule::Number {
                start: 1,
                step: 1,
                pad: 3,
                at: Position::Start,
                separator: " ".into(),
            }],
        ),
        p(
            "Date prefix",
            vec![RenameRule::Insert {
                text: "{date} ".into(),
                at: Position::Start,
                scope: Scope::Name,
            }],
        ),
        p(
            "Spaces to underscores",
            vec![RenameRule::Replace {
                find: " ".into(),
                replace: "_".into(),
                regex: false,
                case_sensitive: false,
                scope: Scope::Name,
            }],
        ),
        p(
            "Web-friendly names",
            vec![
                RenameRule::Case {
                    case: CaseKind::Lower,
                    scope: Scope::Whole,
                },
                RenameRule::Replace {
                    find: r"[^a-z0-9.]+".into(),
                    replace: "-".into(),
                    regex: true,
                    case_sensitive: false,
                    scope: Scope::Name,
                },
                RenameRule::Replace {
                    find: r"^-+|-+$".into(),
                    replace: String::new(),
                    regex: true,
                    case_sensitive: false,
                    scope: Scope::Name,
                },
            ],
        ),
        p(
            "Lower-case extensions",
            vec![RenameRule::Case {
                case: CaseKind::Lower,
                scope: Scope::Extension,
            }],
        ),
    ]
}

/// The user's own presets, persisted as JSON.
pub struct RenamePresets {
    path: Option<PathBuf>,
    items: Mutex<Vec<RenamePreset>>,
}

impl RenamePresets {
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_path_buf();
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            items: Mutex::new(items),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            items: Mutex::new(Vec::new()),
        }
    }

    /// Built-in presets first, then the user's.
    pub fn list(&self) -> Vec<RenamePreset> {
        let mut v = builtin_presets();
        v.extend(self.items.lock().unwrap().iter().cloned());
        v
    }

    /// Save a preset, replacing a user preset with the same name.
    pub fn save(&self, mut preset: RenamePreset) -> Result<()> {
        preset.name = preset.name.trim().to_string();
        if preset.name.is_empty() {
            return Err(Error::Invalid("a preset needs a name".into()));
        }
        if builtin_presets().iter().any(|b| b.name == preset.name) {
            return Err(Error::AlreadyExists(format!(
                "a built-in preset is called {}",
                preset.name
            )));
        }
        Renamer::new(&preset.rules)?;
        preset.builtin = false;
        let mut items = self.items.lock().unwrap();
        match items.iter_mut().find(|p| p.name == preset.name) {
            Some(p) => *p = preset,
            None => items.push(preset),
        }
        self.persist(&items)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut items = self.items.lock().unwrap();
        items.retain(|p| p.name != name);
        self.persist(&items)
    }

    fn persist(&self, items: &[RenamePreset]) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io(dir.to_string_lossy(), e))?;
        }
        let json = serde_json::to_vec_pretty(items).map_err(|e| Error::Invalid(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| Error::io(path.to_string_lossy(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str) -> RenameInput {
        RenameInput {
            name: name.into(),
            dir: false,
            modified_ms: None,
            parent: "Photos".into(),
        }
    }

    fn names(rules: Vec<RenameRule>, files: &[&str]) -> Vec<String> {
        let inputs: Vec<_> = files.iter().map(|f| input(f)).collect();
        preview(&rules, &inputs, &[])
            .unwrap()
            .into_iter()
            .map(|p| p.new_name)
            .collect()
    }

    #[test]
    fn replace_plain_and_regex() {
        let plain = RenameRule::Replace {
            find: "IMG".into(),
            replace: "Trip $1".into(),
            regex: false,
            case_sensitive: false,
            scope: Scope::Name,
        };
        assert_eq!(names(vec![plain], &["img_01.JPG"]), ["Trip $1_01.JPG"]);
        let re = RenameRule::Replace {
            find: r"^IMG_(\d+)$".into(),
            replace: "Trip ${1}".into(),
            regex: true,
            case_sensitive: true,
            scope: Scope::Name,
        };
        assert_eq!(
            names(vec![re], &["IMG_0042.jpg", "img_1.jpg"]),
            ["Trip 0042.jpg", "img_1.jpg"]
        );
    }

    #[test]
    fn case_numbering_insert_remove_and_extension() {
        let rules = vec![
            RenameRule::Case {
                case: CaseKind::Title,
                scope: Scope::Name,
            },
            RenameRule::Case {
                case: CaseKind::Lower,
                scope: Scope::Extension,
            },
            RenameRule::Number {
                start: 9,
                step: 1,
                pad: 3,
                at: Position::End,
                separator: " - ".into(),
            },
            RenameRule::Insert {
                text: "{parent}_".into(),
                at: Position::Start,
                scope: Scope::Name,
            },
        ];
        assert_eq!(
            names(rules, &["my summer TRIP.JPG", "beach.Png", ".hidden"]),
            [
                "Photos_My Summer Trip - 009.jpg",
                "Photos_Beach - 010.png",
                "Photos_.Hidden - 011"
            ]
        );
        let rules = vec![
            RenameRule::Remove {
                count: 4,
                from: Position::Start,
                offset: 0,
                scope: Scope::Name,
            },
            RenameRule::Remove {
                count: 1,
                from: Position::End,
                offset: 1,
                scope: Scope::Name,
            },
            RenameRule::Extension { ext: ".txt".into() },
        ];
        assert_eq!(names(rules, &["IMG_report1.md"]), ["repor1.txt"]);
    }

    #[test]
    fn tokens_and_folders() {
        let rules = vec![RenameRule::Insert {
            text: "{n:2}-{name}-{ext}-{x}".into(),
            at: Position::Start,
            scope: Scope::Whole,
        }];
        let mut folder = input("a.b");
        folder.dir = true;
        let out = preview(&rules, &[input("x.tar.gz"), folder], &[]).unwrap();
        assert_eq!(out[0].new_name, "01-x.tar-gz-{x}x.tar.gz");
        // Folders have no extension.
        assert_eq!(out[1].new_name, "02-a.b--{x}a.b");
    }

    #[test]
    fn issues_are_reported() {
        let rules = vec![RenameRule::Replace {
            find: r"\d".into(),
            replace: String::new(),
            regex: true,
            case_sensitive: false,
            scope: Scope::Name,
        }];
        let inputs = [
            input("a1.txt"),
            input("A2.txt"),
            input("b1.txt"),
            input("123"),
        ];
        let out = preview(&rules, &inputs, &["B.TXT".into()]).unwrap();
        assert_eq!(out[0].issue.as_deref(), Some("same name as another item"));
        assert_eq!(out[1].issue.as_deref(), Some("same name as another item"));
        assert_eq!(
            out[2].issue.as_deref(),
            Some("an item with this name already exists")
        );
        assert_eq!(out[3].issue.as_deref(), Some("empty name"));
        let bad = vec![RenameRule::Replace {
            find: "(".into(),
            replace: String::new(),
            regex: true,
            case_sensitive: false,
            scope: Scope::Name,
        }];
        assert!(preview(&bad, &inputs, &[]).is_err());
    }

    #[test]
    fn presets_round_trip() {
        let dir = std::env::temp_dir().join(format!("dp-presets-{}", uuid::Uuid::new_v4()));
        let path = dir.join("p.json");
        let store = RenamePresets::open(&path);
        let n = store.list().len();
        store
            .save(RenamePreset {
                name: "Mine".into(),
                rules: vec![RenameRule::Extension { ext: "bak".into() }],
                builtin: true,
            })
            .unwrap();
        assert!(store
            .save(RenamePreset {
                name: "Date prefix".into(),
                rules: vec![],
                builtin: false
            })
            .is_err());
        let again = RenamePresets::open(&path);
        assert_eq!(again.list().len(), n + 1);
        assert!(!again.list().last().unwrap().builtin);
        again.remove("Mine").unwrap();
        assert_eq!(RenamePresets::open(&path).list().len(), n);
        let _ = std::fs::remove_dir_all(dir);
    }
}

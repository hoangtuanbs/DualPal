//! Multi Rename cases (Phase 1, §8.10).

use dualpal_core::rename::{
    builtin_presets, name_problem, preview, Position, RenameInput, RenamePresets, RenameRule, Scope,
};

fn input(name: &str) -> RenameInput {
    RenameInput {
        name: name.into(),
        dir: false,
        modified_ms: Some(1_700_000_000_000),
        parent: "folder".into(),
    }
}

/// REN-006 — every built-in preset applies to the small-tree names cleanly.
#[test]
fn builtin_presets_apply_without_issues() {
    let names = ["a.txt", "b.txt", "c.txt"];
    let inputs: Vec<RenameInput> = names.iter().map(|n| input(n)).collect();
    assert!(!builtin_presets().is_empty(), "the app ships presets");
    for preset in builtin_presets() {
        let out = preview(&preset.rules, &inputs, &[]).unwrap();
        assert!(
            out.iter().all(|p| p.issue.is_none()),
            "preset {:?} produced issues: {out:?}",
            preset.name
        );
    }
}

/// REN-007 — an invalid regular expression is an issue, not a panic.
#[test]
fn invalid_regex_is_an_issue() {
    let rules = [RenameRule::Replace {
        find: "(".into(),
        replace: "x".into(),
        regex: true,
        case_sensitive: false,
        scope: Scope::Name,
    }];
    assert!(preview(&rules, &[input("a.txt")], &[]).is_err());
}

/// REN-008 — the scope of a replace rule (name, extension, whole).
#[test]
fn replace_scope_name_extension_whole() {
    let replace = |scope: Scope| {
        [RenameRule::Replace {
            find: "a".into(),
            replace: "X".into(),
            regex: false,
            case_sensitive: false,
            scope,
        }]
    };
    let whole = input("alpha.bak");
    let name = preview(&replace(Scope::Name), std::slice::from_ref(&whole), &[]).unwrap();
    assert_eq!(name[0].new_name, "XlphX.bak");
    let ext = preview(
        &replace(Scope::Extension),
        std::slice::from_ref(&whole),
        &[],
    )
    .unwrap();
    assert_eq!(ext[0].new_name, "alpha.bXk");
    let all = preview(&replace(Scope::Whole), std::slice::from_ref(&whole), &[]).unwrap();
    assert_eq!(all[0].new_name, "XlphX.bXk");
    // Name scope leaves the extension alone.
    assert_eq!(
        preview(&replace(Scope::Name), &[input("a.b")], &[]).unwrap()[0].new_name,
        "X.b"
    );
}

/// REN-009 — `{date}` is the item mtime in `YYYY-MM-DD` form.
#[test]
fn date_token_uses_mtime() {
    use chrono::TimeZone;
    let ms = 1_700_000_000_000i64;
    let expected = chrono::Local
        .timestamp_millis_opt(ms)
        .earliest()
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let rules = [RenameRule::Insert {
        text: "{date}-".into(),
        at: Position::Start,
        scope: Scope::Name,
    }];
    let out = preview(&rules, &[input("a.txt")], &[]).unwrap();
    assert_eq!(out[0].new_name, format!("{expected}-a.txt"));
}

/// REN-012 — a preview never yields two equal targets without flagging one.
#[test]
fn preview_flags_collisions() {
    proptest::proptest!(|(names in proptest::collection::vec("[a-zA-Z]{1,4}", 1..8))| {
        let mut names = names;
        names.push("a".into());
        let inputs: Vec<RenameInput> = names.iter().map(|n| input(n)).collect();
        let rules = [RenameRule::Replace {
            find: "a".into(),
            replace: "b".into(),
            regex: false,
            case_sensitive: false,
            scope: Scope::Whole,
        }];
        let Ok(out) = preview(&rules, &inputs, &[]) else { return Ok(()) };
        for (i, p) in out.iter().enumerate() {
            for q in &out[i + 1..] {
                if p.new_name.eq_ignore_ascii_case(&q.new_name) {
                    assert!(p.issue.is_some() || q.issue.is_some(), "{out:?}");
                }
            }
        }
    });
}

/// REN-004 support — `name_problem` flags unusable names (sanity).
#[test]
fn name_problem_table() {
    assert_eq!(name_problem(""), Some("empty name"));
    assert_eq!(name_problem(".."), Some("not a valid name"));
    assert_eq!(name_problem("a/b"), Some("contains / or \\"));
    assert!(name_problem("ok.txt").is_none());
}

/// REN-010 — numbering pads to width and starts from 0.
#[test]
fn number_padding_and_start() {
    let rules = [RenameRule::Number {
        start: 0,
        step: 1,
        pad: 3,
        at: Position::Start,
        separator: " ".into(),
    }];
    let out = preview(
        &rules,
        &[input("a.txt"), input("b.txt"), input("c.txt")],
        &[],
    )
    .unwrap();
    assert_eq!(out[0].new_name, "000 a.txt");
    assert_eq!(out[1].new_name, "001 b.txt");
    assert_eq!(out[2].new_name, "002 c.txt");
}

/// REN-011 — rule order matters and survives a JSON round trip.
#[test]
fn rule_order_is_preserved() {
    use dualpal_core::rename::{CaseKind, RenamePreset};
    let ab = RenamePreset {
        name: "ab".into(),
        rules: vec![
            RenameRule::Replace {
                find: "a".into(),
                replace: "b".into(),
                regex: false,
                case_sensitive: false,
                scope: Scope::Whole,
            },
            RenameRule::Replace {
                find: "b".into(),
                replace: "c".into(),
                regex: false,
                case_sensitive: false,
                scope: Scope::Whole,
            },
        ],
        builtin: false,
    };
    let json = serde_json::to_string(&ab).unwrap();
    let back: RenamePreset = serde_json::from_str(&json).unwrap();
    assert_eq!(back.rules, ab.rules);

    // Order matters: a→b then b→c gives "c", the reverse gives "b".
    let forward = preview(&ab.rules, &[input("a.txt")], &[]).unwrap();
    let reversed = preview(
        &ab.rules.iter().rev().cloned().collect::<Vec<_>>(),
        &[input("a.txt")],
        &[],
    )
    .unwrap();
    assert_eq!(forward[0].new_name, "c.txt");
    assert_eq!(reversed[0].new_name, "b.txt");
    let _ = CaseKind::Lower;
}

/// REN-014 — saving a preset replaces by name, and removal works.
#[test]
fn presets_save_replace_and_remove() {
    let p = RenamePresets::in_memory();
    p.save(dualpal_core::rename::RenamePreset {
        name: "My rule".into(),
        rules: vec![],
        builtin: false,
    })
    .unwrap();
    p.save(dualpal_core::rename::RenamePreset {
        name: "My rule".into(),
        rules: vec![RenameRule::Case {
            case: dualpal_core::rename::CaseKind::Upper,
            scope: Scope::Whole,
        }],
        builtin: false,
    })
    .unwrap();
    let mine: Vec<_> = p.list().into_iter().filter(|x| !x.builtin).collect();
    assert_eq!(mine.len(), 1, "replacing by name, not duplicating");
    assert_eq!(mine[0].rules.len(), 1);

    p.remove("My rule").unwrap();
    assert!(p.list().iter().all(|x| x.builtin));
}

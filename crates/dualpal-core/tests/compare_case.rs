//! CMP-013 — names that differ only in case, between a case-insensitive
//! local disk and a case-sensitive server (§8.7).
//!
//! Documented behaviour: Compare matches names exactly, so `Case.txt` on one
//! side and `case.txt` on the other are two different items, one on each
//! side only. Sync leaves such items for the user (a case conflict), since
//! on a case-insensitive disk copying one would overwrite the other.

use std::collections::HashMap;
use std::sync::Arc;

use dualpal_core::compare::{compare, CompareOptions, DiffState};
use dualpal_core::sync::{SyncAction, SyncEngine, SyncMode, SyncReason};
use dualpal_core::{ActivityLog, CancellationToken, Location, OperationQueue, Registry};
use dualpal_testkit::MemFs;

fn setup(server: &MemFs) -> Arc<Registry> {
    let registry = Arc::new(Registry::with_local());
    registry.register(Arc::new(server.clone()));
    registry
}

/// CMP-013 — `Case.txt` locally and `case.txt` on the server are listed as
/// two items, each on one side only.
#[tokio::test]
async fn names_differing_in_case_are_two_items() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Case.txt"), "local").unwrap();
    let server = MemFs::new("server");
    server.seed_file("/r/case.txt", "server");
    let registry = setup(&server);

    let c = compare(
        &registry,
        &Location::new("local", tmp.path().to_string_lossy()),
        &Location::new("server", "/r"),
        CompareOptions::default(),
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    let mut items: Vec<(&str, DiffState)> =
        c.items.iter().map(|i| (i.rel.as_str(), i.state)).collect();
    items.sort_by_key(|(rel, _)| rel.to_string());
    assert_eq!(
        items,
        [
            ("Case.txt", DiffState::LeftOnly),
            ("case.txt", DiffState::RightOnly)
        ]
    );
}

/// CMP-013 — on a case-insensitive local disk, a two-way sync of those two
/// items must not replace the local file with the server's one.
#[tokio::test]
async fn two_way_sync_does_not_overwrite_a_name_that_differs_in_case() {
    let tmp = tempfile::tempdir().unwrap();
    let probe = tmp.path().join("probe");
    std::fs::write(&probe, "x").unwrap();
    let insensitive = tmp.path().join("PROBE").exists();
    std::fs::remove_file(&probe).unwrap();
    if !insensitive {
        eprintln!("skipped: this disk is case-sensitive");
        return;
    }

    std::fs::write(tmp.path().join("Case.txt"), "local").unwrap();
    let server = MemFs::new("server");
    server.seed_file("/r/case.txt", "server");
    let registry = setup(&server);
    let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(50)), 2);
    let engine = SyncEngine::new(q.clone(), None);
    let (l, r) = (
        Location::new("local", tmp.path().to_string_lossy()),
        Location::new("server", "/r"),
    );

    let p = engine
        .plan(
            &l,
            &r,
            SyncMode::TwoWay,
            CompareOptions::default(),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let run = engine.run(p.id, &HashMap::new()).unwrap();
    run.done.await.unwrap();

    let local: Vec<(String, String)> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read_to_string(e.path()).unwrap(),
            )
        })
        .collect();
    assert!(
        local.iter().any(|(_, content)| content == "local"),
        "the local file's content was lost: {local:?}"
    );
}

/// CMP-013 — in every mode, a sync plans to skip both names as a conflict
/// instead of copying one over the other.
#[tokio::test]
async fn sync_plans_names_differing_in_case_as_conflicts() {
    for mode in [SyncMode::Update, SyncMode::Mirror, SyncMode::TwoWay] {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Case.txt"), "local").unwrap();
        std::fs::write(tmp.path().join("other.txt"), "x").unwrap();
        let server = MemFs::new("server");
        server.seed_file("/r/case.txt", "server");
        let registry = setup(&server);
        let q = OperationQueue::new(registry, Arc::new(ActivityLog::in_memory(50)), 2);
        let engine = SyncEngine::new(q, None);
        let p = engine
            .plan(
                &Location::new("local", tmp.path().to_string_lossy()),
                &Location::new("server", "/r"),
                mode,
                CompareOptions::default(),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        let mut steps: Vec<(&str, SyncAction, SyncReason, bool)> = p
            .steps
            .iter()
            .map(|s| (s.rel.as_str(), s.action, s.reason, s.conflict))
            .collect();
        steps.sort_by_key(|s| s.0.to_string());
        assert_eq!(
            steps,
            [
                ("Case.txt", SyncAction::Skip, SyncReason::CaseConflict, true),
                ("case.txt", SyncAction::Skip, SyncReason::CaseConflict, true),
                ("other.txt", SyncAction::CopyToRight, SyncReason::New, false),
            ],
            "{mode:?}"
        );
    }
}

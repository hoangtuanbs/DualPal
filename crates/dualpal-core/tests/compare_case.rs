//! CMP-013 — names that differ only in case, between a case-insensitive
//! local disk and a case-sensitive server (§8.7).
//!
//! Documented behaviour: Compare matches names exactly, so `Case.txt` on one
//! side and `case.txt` on the other are two different items, one on each
//! side only.

use std::collections::HashMap;
use std::sync::Arc;

use dualpal_core::compare::{compare, CompareOptions, DiffState};
use dualpal_core::sync::{SyncEngine, SyncMode};
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
#[ignore = "BUG: on a case-insensitive disk a two-way sync copies the server's case.txt over the local Case.txt, losing its content"]
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

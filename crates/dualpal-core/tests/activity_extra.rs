//! Activity log and insights cases (Phase 1, §8.4).

use dualpal_core::activity::{insights, ActivityLog, ActivityRecord};
use dualpal_core::OperationStatus;
use uuid::Uuid;

fn record(
    kind: &str,
    status: OperationStatus,
    finished_ms: i64,
    files: u64,
    bytes: u64,
) -> ActivityRecord {
    ActivityRecord {
        id: Uuid::new_v4(),
        kind: kind.into(),
        summary: format!("{kind} test"),
        status,
        started_ms: finished_ms - 1000,
        finished_ms,
        files_done: files,
        bytes_done: bytes,
        skipped: 0,
        error: (status == OperationStatus::Failed).then(|| "boom".to_string()),
        places: vec!["local".into()],
    }
}

/// ACT-003 — a half-written last line is skipped, valid records kept.
#[test]
fn corrupt_line_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("activity.jsonl");
    let good = record("copy", OperationStatus::Completed, 1000, 1, 1);
    std::fs::write(
        &path,
        format!(
            "{}\n{{\"id\":\"truncat",
            serde_json::to_string(&good).unwrap()
        ),
    )
    .unwrap();
    let log = ActivityLog::open(&path, 10).unwrap();
    let list = log.list();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].kind, "copy");
}

/// ACT-004 — clear empties both the memory and the file.
#[test]
fn clear_empties_file_and_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("activity.jsonl");
    let log = ActivityLog::open(&path, 10).unwrap();
    log.record(record("copy", OperationStatus::Completed, 1000, 1, 1));
    assert_eq!(log.list().len(), 1);
    log.clear().unwrap();
    assert!(log.list().is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

/// ACT-005 — insights count by kind and place exactly.
#[test]
fn insights_count_by_kind_and_place() {
    let recs = vec![
        record(
            "copy",
            OperationStatus::Completed,
            1_700_000_000_000,
            2,
            200,
        ),
        record("copy", OperationStatus::Failed, 1_700_000_000_000, 0, 0),
        record("move", OperationStatus::Cancelled, 1_700_000_000_000, 0, 0),
    ];
    let ins = insights(&recs, 1_700_000_100_000, 7, 0);
    assert_eq!(ins.operations, 3);
    assert_eq!(ins.completed, 1);
    assert_eq!(ins.failed, 1);
    assert_eq!(ins.cancelled, 1);
    assert_eq!(ins.files, 2);
    assert_eq!(ins.bytes, 200);
    let copy = ins.by_kind.iter().find(|g| g.key == "copy").unwrap();
    assert_eq!(copy.operations, 2);
    assert_eq!(copy.failed, 1);
    assert_eq!(ins.failures.len(), 1);
    assert_eq!(ins.failures[0].kind, "copy");
}

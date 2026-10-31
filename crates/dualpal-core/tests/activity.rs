use dualpal_core::{ActivityLog, ActivityRecord, OperationStatus};

fn rec(n: u64) -> ActivityRecord {
    ActivityRecord {
        id: uuid::Uuid::new_v4(),
        kind: "copy".into(),
        summary: format!("op {n}"),
        status: OperationStatus::Completed,
        started_ms: n as i64,
        finished_ms: n as i64,
        files_done: n,
        bytes_done: 0,
        skipped: 0,
        error: None,
    }
}

#[test]
fn persists_newest_first_and_respects_capacity() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("logs/activity.jsonl");
    {
        let log = ActivityLog::open(&path, 3).unwrap();
        for n in 0..5 {
            log.record(rec(n));
        }
        let summaries: Vec<_> = log.list().into_iter().map(|r| r.summary).collect();
        assert_eq!(summaries, ["op 4", "op 3", "op 2"]);
    }
    let reopened = ActivityLog::open(&path, 3).unwrap();
    let summaries: Vec<_> = reopened.list().into_iter().map(|r| r.summary).collect();
    assert_eq!(summaries, ["op 4", "op 3", "op 2"]);
    // Reopening compacts the file to the retained records.
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 3);

    reopened.clear().unwrap();
    assert!(ActivityLog::open(&path, 3).unwrap().list().is_empty());
}

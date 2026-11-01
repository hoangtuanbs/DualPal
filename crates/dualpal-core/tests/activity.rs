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
        places: vec!["local".into()],
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

#[test]
fn insights_bucket_by_local_day() {
    use dualpal_core::activity::insights;
    const H: i64 = 3_600_000;
    let day0 = 20_000 * 24 * H; // a UTC midnight
    let mk =
        |kind: &str, status, start: i64, end: i64, bytes: u64, places: &[&str]| ActivityRecord {
            kind: kind.into(),
            status,
            started_ms: start,
            finished_ms: end,
            bytes_done: bytes,
            files_done: 1,
            places: places.iter().map(|s| s.to_string()).collect(),
            ..rec(0)
        };
    let recs = vec![
        mk(
            "copy",
            OperationStatus::Completed,
            day0 + 10 * H,
            day0 + 10 * H + 2000,
            4000,
            &["local", "conn:a"],
        ),
        // 23:30 UTC, which is the next day at UTC+1.
        mk(
            "sync",
            OperationStatus::Completed,
            day0 + 23 * H,
            day0 + 23 * H + 1_800_000,
            0,
            &["conn:a"],
        ),
        mk(
            "delete",
            OperationStatus::Failed,
            day0 + 30 * H,
            day0 + 30 * H,
            0,
            &["local"],
        ),
        // Too old for a 3 day window.
        mk(
            "copy",
            OperationStatus::Completed,
            day0 - 5 * 24 * H,
            day0 - 5 * 24 * H,
            99,
            &["local"],
        ),
    ];
    let now = day0 + 40 * H;
    let i = insights(&recs, now, 3, 60);
    assert_eq!(i.days.len(), 3);
    assert_eq!(i.operations, 3);
    assert_eq!(i.failed, 1);
    assert_eq!(i.bytes, 4000);
    assert_eq!(
        i.days.iter().map(|d| d.operations).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(i.days[2].start_ms, day0 + 23 * H);
    assert_eq!(i.throughput, Some(2000.0));
    assert_eq!(i.by_kind[0].key, "copy");
    assert_eq!(
        i.by_place
            .iter()
            .find(|g| g.key == "conn:a")
            .unwrap()
            .operations,
        2
    );
    assert_eq!(i.largest.len(), 1);
    assert_eq!(i.failures[0].kind, "delete");
}

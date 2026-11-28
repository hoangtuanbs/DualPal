//! Transfer queue upgrades: pause and resume, operations kept across a
//! restart, bandwidth limits, checking copies, and "don't stop on errors".

use std::sync::Arc;
use std::time::Duration;

use dualpal_core::ops::{failure_report, RateLimiter};
use dualpal_core::{
    ActivityLog, ConflictPolicy, Location, Operation, OperationInfo, OperationQueue,
    OperationStatus, Registry, TransferOptions,
};
use dualpal_testkit::{FaultyFs, MemFs};

const WAIT: Duration = Duration::from_secs(20);

struct Setup {
    registry: Arc<Registry>,
    src: Arc<MemFs>,
    dst: Arc<FaultyFs<MemFs>>,
}

/// A source and a destination in memory; the destination is "remote" and
/// writes under temporary names (no atomic writes).
fn setup() -> Setup {
    let registry = Arc::new(Registry::new());
    let src = Arc::new(MemFs::new("src"));
    src.seed_dir("/in");
    let dst = Arc::new(FaultyFs::new(MemFs::new("dst")));
    dst.inner().seed_dir("/out");
    dst.set_atomic_writes(false);
    registry.register(src.clone());
    registry.register(dst.clone());
    Setup { registry, src, dst }
}

fn queue(reg: &Arc<Registry>, slots: usize) -> OperationQueue {
    OperationQueue::new(reg.clone(), Arc::new(ActivityLog::in_memory(100)), slots)
}

fn copy(names: &[&str]) -> Operation {
    Operation::Copy {
        sources: names
            .iter()
            .map(|n| Location::new("src", format!("/in/{n}")))
            .collect(),
        destination: Location::new("dst", "/out"),
        conflict: ConflictPolicy::Fail,
    }
}

fn limited(bytes_per_sec: u64) -> TransferOptions {
    TransferOptions {
        limit: Some(bytes_per_sec),
        ..Default::default()
    }
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

fn parts(fs: &MemFs) -> Vec<String> {
    fs.paths()
        .into_iter()
        .filter(|p| p.ends_with(".dualpal-part"))
        .collect()
}

async fn wait_for(
    q: &OperationQueue,
    id: uuid::Uuid,
    f: impl Fn(&OperationInfo) -> bool,
) -> OperationInfo {
    tokio::time::timeout(WAIT, q.wait_for(id, f))
        .await
        .expect("timed out")
        .unwrap()
}

async fn finished(q: &OperationQueue, id: uuid::Uuid) -> OperationInfo {
    wait_for(q, id, |i| i.status.is_finished()).await
}

/// QUEUE-001 — pausing a copy mid-file stops it after the chunk in flight;
/// resuming carries on in the same file without starting it again.
#[tokio::test]
async fn pause_and_resume_mid_file() {
    let s = setup();
    let body = data(2 << 20);
    s.src.seed_file("/in/big.bin", &body);
    let q = queue(&s.registry, 2);
    let id = q.enqueue_with(copy(&["big.bin"]), limited(4 << 20));
    wait_for(&q, id, |i| i.progress.bytes_done >= 256 * 1024).await;
    assert!(q.pause(id));
    let paused = wait_for(&q, id, |i| i.status == OperationStatus::Paused).await;
    assert!(paused.progress.bytes_done < body.len() as u64, "{paused:?}");
    let before = q.get(id).unwrap().progress.bytes_done;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let still = q.get(id).unwrap();
    assert_eq!(still.status, OperationStatus::Paused);
    assert_eq!(
        still.progress.bytes_done, before,
        "nothing is written while paused"
    );
    assert_eq!(parts(s.dst.inner()).len(), 1, "the partial file stays");

    assert!(q.resume(id));
    let done = finished(&q, id).await;
    assert_eq!(done.status, OperationStatus::Completed, "{done:?}");
    assert_eq!(s.dst.inner().contents("/out/big.bin").unwrap(), body);
    assert!(
        s.dst.resumed_offsets().is_empty(),
        "the file was not reopened"
    );
    assert!(parts(s.dst.inner()).is_empty());
    // Unknown or finished operations can't be paused.
    assert!(!q.pause(id));
    assert!(!q.resume(uuid::Uuid::new_v4()));
}

/// QUEUE-002 — a paused operation gives its slot to the next one, and a
/// queued one paused before it starts stays put until resumed.
#[tokio::test]
async fn paused_operations_free_their_slot() {
    let s = setup();
    s.src.seed_file("/in/big.bin", data(1 << 20));
    s.src.seed_file("/in/small.txt", "small");
    s.src.seed_file("/in/later.txt", "later");
    let q = queue(&s.registry, 1);
    let big = q.enqueue_with(copy(&["big.bin"]), limited(1 << 20));
    let later = q.enqueue(copy(&["later.txt"]));
    assert!(q.pause(later));
    wait_for(&q, big, |i| {
        i.status == OperationStatus::Running && i.progress.bytes_done > 0
    })
    .await;
    q.pause(big);
    wait_for(&q, big, |i| i.status == OperationStatus::Paused).await;

    let small = q.enqueue(copy(&["small.txt"]));
    assert_eq!(finished(&q, small).await.status, OperationStatus::Completed);
    assert_eq!(q.get(later).unwrap().status, OperationStatus::Paused);

    q.resume(big);
    assert_eq!(finished(&q, big).await.status, OperationStatus::Completed);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(q.get(later).unwrap().status, OperationStatus::Paused);
    q.resume(later);
    assert_eq!(finished(&q, later).await.status, OperationStatus::Completed);
    assert_eq!(s.dst.inner().contents("/out/later.txt").unwrap(), b"later");
}

/// QUEUE-003 — pausing the whole queue holds every running operation and
/// keeps queued ones from starting; resuming lets them all go on.
#[tokio::test]
async fn pause_the_whole_queue() {
    let s = setup();
    s.src.seed_file("/in/a.bin", data(1 << 20));
    s.src.seed_file("/in/b.txt", "b");
    let q = queue(&s.registry, 1);
    let a = q.enqueue_with(copy(&["a.bin"]), limited(1 << 20));
    let b = q.enqueue(copy(&["b.txt"]));
    wait_for(&q, a, |i| i.progress.bytes_done > 0).await;
    q.set_queue_paused(true);
    assert!(q.queue_paused());
    wait_for(&q, a, |i| i.status == OperationStatus::Paused).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        q.get(b).unwrap().status,
        OperationStatus::Queued,
        "b never started"
    );
    q.set_queue_paused(false);
    assert_eq!(finished(&q, a).await.status, OperationStatus::Completed);
    assert_eq!(finished(&q, b).await.status, OperationStatus::Completed);
}

/// QUEUE-004 — an unfinished copy is kept in the store with its partial
/// file; after a "restart" it comes back paused and restored, and resuming
/// carries on in the partial file where the target allows it.
#[tokio::test]
async fn copies_survive_a_restart_and_resume_mid_file() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("queue.json");
    let body = data(8 << 20);
    s.src.seed_file("/in/big.bin", &body);
    s.src.seed_file("/in/small.txt", "small");

    let q1 = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        &store,
    );
    // Other kinds of operation are not kept.
    q1.enqueue(Operation::CreateDir {
        location: Location::new("dst", "/out/new"),
    });
    let id = q1.enqueue_with(copy(&["small.txt", "big.bin"]), limited(32 << 20));
    wait_for(&q1, id, |i| i.progress.bytes_done >= 6 << 20).await;
    q1.pause(id);
    wait_for(&q1, id, |i| i.status == OperationStatus::Paused).await;
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    let ops = saved["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 1, "{saved}");
    assert_eq!(ops[0]["id"], id.to_string());
    assert_eq!(ops[0]["options"]["limit"], 32 << 20);
    assert_eq!(ops[0]["parts"].as_object().unwrap().len(), 1, "{saved}");
    assert!(ops[0]["parts"]["/out/big.bin"]
        .as_str()
        .unwrap()
        .ends_with(".dualpal-part"));

    // The app closes (q1 is left behind, never resumed) and starts again.
    let q2 = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        &store,
    );
    assert_eq!(q2.restore(), 1);
    assert_eq!(q2.restore(), 0, "restoring twice adds nothing");
    let info = q2.get(id).unwrap();
    assert_eq!(info.status, OperationStatus::Paused);
    assert!(info.restored);
    assert_eq!(info.waiting_for, None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        q2.get(id).unwrap().status,
        OperationStatus::Paused,
        "never runs by itself"
    );

    assert!(q2.resume(id));
    let done = finished(&q2, id).await;
    assert_eq!(done.status, OperationStatus::Completed, "{done:?}");
    assert_eq!(s.dst.inner().contents("/out/big.bin").unwrap(), body);
    assert_eq!(s.dst.inner().contents("/out/small.txt").unwrap(), b"small");
    let offsets = s.dst.resumed_offsets();
    assert!(offsets.len() == 1 && offsets[0] > 0, "{offsets:?}");
    assert!(parts(s.dst.inner()).is_empty());
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert_eq!(saved["operations"].as_array().unwrap().len(), 0);
}

/// Start a copy of `big.bin` with a store, pause it midway and return a
/// second queue over the same store with the copy restored.
async fn restored(s: &Setup, store: &std::path::Path) -> (OperationQueue, uuid::Uuid) {
    s.src.seed_file("/in/big.bin", data(2 << 20));
    let q1 = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        store,
    );
    let id = q1.enqueue_with(copy(&["big.bin"]), limited(8 << 20));
    wait_for(&q1, id, |i| i.progress.bytes_done >= 512 * 1024).await;
    q1.pause(id);
    wait_for(&q1, id, |i| i.status == OperationStatus::Paused).await;
    let q2 = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        store,
    );
    assert_eq!(q2.restore(), 1);
    (q2, id)
}

/// QUEUE-005 — a target that can't be written midway starts the file again.
#[tokio::test]
async fn restart_on_a_non_resumable_target_starts_the_file_again() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let (q, id) = restored(&s, &tmp.path().join("queue.json")).await;
    s.dst.set_resumable(false);
    q.resume(id);
    assert_eq!(finished(&q, id).await.status, OperationStatus::Completed);
    assert!(s.dst.resumed_offsets().is_empty());
    assert_eq!(
        s.dst.inner().contents("/out/big.bin").unwrap(),
        data(2 << 20)
    );
    assert!(parts(s.dst.inner()).is_empty());
}

/// QUEUE-006 — discarding a restored copy removes its partial file and
/// forgets it.
#[tokio::test]
async fn discarding_a_restored_copy_cleans_up() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("queue.json");
    let (q, id) = restored(&s, &store).await;
    assert_eq!(parts(s.dst.inner()).len(), 1);
    assert!(q.cancel(id));
    assert_eq!(finished(&q, id).await.status, OperationStatus::Cancelled);
    assert!(parts(s.dst.inner()).is_empty());
    assert!(s.dst.inner().contents("/out/big.bin").is_none());
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert_eq!(saved["operations"].as_array().unwrap().len(), 0);
}

/// QUEUE-007 — a restored operation whose server is not connected says so,
/// and once resumed starts by itself when the server connects.
#[tokio::test]
async fn restored_operations_wait_for_their_server() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("queue.json");
    let id = uuid::Uuid::new_v4();
    let saved = serde_json::json!({ "operations": [{
        "id": id,
        "operation": {
            "type": "copy",
            "sources": [{ "fs": "later", "path": "/a.txt" }],
            "destination": { "fs": "dst", "path": "/out" },
        },
        "queuedMs": 1,
    }]});
    std::fs::write(&store, saved.to_string()).unwrap();
    let q = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        &store,
    );
    assert_eq!(q.restore(), 1);
    assert_eq!(q.get(id).unwrap().waiting_for.as_deref(), Some("later"));
    q.resume(id);
    let waiting = wait_for(&q, id, |i| i.status == OperationStatus::Queued).await;
    assert_eq!(waiting.waiting_for.as_deref(), Some("later"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(q.get(id).unwrap().status, OperationStatus::Queued);

    let later = Arc::new(MemFs::new("later"));
    later.seed_file("/a.txt", "here");
    s.registry.register(later);
    let done = finished(&q, id).await;
    assert_eq!(done.status, OperationStatus::Completed, "{done:?}");
    assert_eq!(done.waiting_for, None);
    assert_eq!(s.dst.inner().contents("/out/a.txt").unwrap(), b"here");
}

/// QUEUE-008 — refresh_waiting updates restored operations that have not
/// been resumed when a server connects.
#[tokio::test]
async fn refresh_waiting_notices_a_connected_server() {
    let s = setup();
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("queue.json");
    let id = uuid::Uuid::new_v4();
    let saved = serde_json::json!({ "operations": [{
        "id": id,
        "operation": {
            "type": "move",
            "sources": [{ "fs": "later", "path": "/a.txt" }],
            "destination": { "fs": "dst", "path": "/out" },
        },
        "queuedMs": 1,
    }]});
    std::fs::write(&store, saved.to_string()).unwrap();
    let q = OperationQueue::with_store(
        s.registry.clone(),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        &store,
    );
    q.restore();
    assert_eq!(q.get(id).unwrap().waiting_for.as_deref(), Some("later"));
    s.registry.register(Arc::new(MemFs::new("later")));
    q.refresh_waiting();
    assert_eq!(q.get(id).unwrap().waiting_for, None);
}

/// QUEUE-009 — the token bucket keeps to its rate (on tokio's paused clock).
#[tokio::test(start_paused = true)]
async fn limiter_keeps_to_its_rate() {
    let l = RateLimiter::new(1000);
    let start = tokio::time::Instant::now();
    for _ in 0..10 {
        tokio::time::sleep(l.reserve(500)).await;
    }
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(4900) && took <= Duration::from_millis(5100),
        "{took:?}"
    );
    // Saved-up time is capped at one second's worth.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(l.reserve(1000), Duration::ZERO);
    assert!(l.reserve(1000) >= Duration::from_millis(999));
    l.set_rate(0);
    assert_eq!(l.reserve(1 << 30), Duration::ZERO);
}

/// QUEUE-010 — the global limit is shared by running copies; an operation's
/// own limit of 0 ignores it.
#[tokio::test(start_paused = true)]
async fn global_limit_is_shared_by_running_copies() {
    let s = setup();
    s.src.seed_file("/in/a.bin", data(32 * 1024));
    s.src.seed_file("/in/b.bin", data(32 * 1024));
    s.src.seed_file("/in/c.bin", data(256 * 1024));
    let q = queue(&s.registry, 2);
    q.set_bandwidth_limit(32 * 1024);
    assert_eq!(q.bandwidth_limit(), 32 * 1024);

    let start = tokio::time::Instant::now();
    let a = q.enqueue(copy(&["a.bin"]));
    let b = q.enqueue(copy(&["b.bin"]));
    finished(&q, a).await;
    finished(&q, b).await;
    let took = start.elapsed();
    // 64 KiB at 32 KiB/s together: two seconds, not one.
    assert!(
        took >= Duration::from_millis(1800) && took <= Duration::from_millis(2500),
        "{took:?}"
    );

    let start = tokio::time::Instant::now();
    let c = q.enqueue_with(copy(&["c.bin"]), limited(0));
    assert_eq!(finished(&q, c).await.status, OperationStatus::Completed);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
}

/// QUEUE-011 — an operation's limit can be changed while it runs.
#[tokio::test]
async fn operation_limit_changes_while_running() {
    let s = setup();
    s.src.seed_file("/in/a.bin", data(1 << 20));
    let q = queue(&s.registry, 2);
    let start = std::time::Instant::now();
    let a = q.enqueue_with(copy(&["a.bin"]), limited(16 * 1024));
    wait_for(&q, a, |i| i.progress.bytes_done > 0).await;
    assert!(q.set_operation_limit(a, Some(0)));
    let done = finished(&q, a).await;
    assert_eq!(done.status, OperationStatus::Completed);
    assert_eq!(done.options.limit, Some(0));
    // At 16 KiB/s the whole file would take a minute.
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

fn verified() -> TransferOptions {
    TransferOptions {
        verify: true,
        ..Default::default()
    }
}

/// QUEUE-012 — a checked copy whose written bytes differ fails, and leaves
/// neither the bad file nor a temporary one.
#[tokio::test]
async fn verify_catches_a_corrupt_copy() {
    let s = setup();
    s.src.seed_file("/in/a.txt", "hello world");
    s.dst.corrupt_writes(true);
    let q = queue(&s.registry, 2);
    let id = q.enqueue_with(copy(&["a.txt"]), verified());
    let info = finished(&q, id).await;
    assert_eq!(info.status, OperationStatus::Failed, "{info:?}");
    let err = info.error.unwrap();
    assert!(err.message.contains("does not match"), "{err:?}");
    assert!(s.dst.inner().contents("/out/a.txt").is_none());
    assert!(parts(s.dst.inner()).is_empty());

    // Unchecked, the same copy "succeeds".
    let id = q.enqueue(copy(&["a.txt"]));
    assert_eq!(finished(&q, id).await.status, OperationStatus::Completed);
    assert_ne!(
        s.dst.inner().contents("/out/a.txt").unwrap(),
        b"hello world"
    );
}

/// QUEUE-013 — good copies pass the check; where reading back is expensive
/// the check compares sizes only.
#[tokio::test]
async fn verify_passes_good_copies_and_falls_back_to_size() {
    let s = setup();
    s.src.seed_file("/in/a.txt", "hello world");
    let q = queue(&s.registry, 2);
    let id = q.enqueue_with(copy(&["a.txt"]), verified());
    assert_eq!(finished(&q, id).await.status, OperationStatus::Completed);
    assert_eq!(
        s.dst.inner().contents("/out/a.txt").unwrap(),
        b"hello world"
    );

    s.dst.set_checksum_reads(false);
    s.dst.corrupt_writes(true);
    s.src.seed_file("/in/b.txt", "same size");
    let id = q.enqueue_with(copy(&["b.txt"]), verified());
    assert_eq!(
        finished(&q, id).await.status,
        OperationStatus::Completed,
        "size matches"
    );
}

/// QUEUE-014 — "don't stop on errors": failed items are skipped and listed,
/// the rest is copied, and the failures can be retried as a new operation.
#[tokio::test]
async fn unattended_copy_lists_failures_and_retries_them() {
    let s = setup();
    for n in ["a.txt", "b.txt", "c.txt"] {
        s.src.seed_file(&format!("/in/{n}"), n);
    }
    s.src.seed_dir("/in/sub");
    s.src.seed_file("/in/sub/d.txt", "d");
    s.dst.inner().seed_file("/out/b.txt", "old");
    s.dst.inner().seed_dir("/out/sub");
    s.dst.inner().seed_file("/out/sub/d.txt", "old d");
    let q = queue(&s.registry, 2);
    let op = copy(&["a.txt", "b.txt", "missing.txt", "c.txt", "sub"]);
    let keep_going = TransferOptions {
        keep_going: true,
        ..Default::default()
    };
    let id = q.enqueue_with(op, keep_going.clone());
    let info = finished(&q, id).await;
    assert_eq!(info.status, OperationStatus::Completed, "{info:?}");
    assert_eq!(info.failed, 3);
    assert!(info.options.keep_going);
    assert_eq!(s.dst.inner().contents("/out/a.txt").unwrap(), b"a.txt");
    assert_eq!(s.dst.inner().contents("/out/c.txt").unwrap(), b"c.txt");
    assert_eq!(s.dst.inner().contents("/out/b.txt").unwrap(), b"old");

    let failed = q.failed_items(id);
    let mut paths: Vec<_> = failed.iter().map(|f| f.source.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["/in/b.txt", "/in/missing.txt", "/in/sub/d.txt"]);
    let b = failed
        .iter()
        .find(|f| f.source.path == "/in/b.txt")
        .unwrap();
    assert_eq!(b.error.kind, "alreadyExists");
    assert_eq!(b.target, Location::new("dst", "/out/b.txt"));
    let missing = failed
        .iter()
        .find(|f| f.source.path == "/in/missing.txt")
        .unwrap();
    assert_eq!(missing.error.kind, "notFound");
    let report = failure_report(&failed);
    assert_eq!(report.lines().count(), 3);
    assert!(report.contains("/in/b.txt: already exists"), "{report}");

    let log = q.activity().list();
    assert_eq!(log[0].error.as_deref(), Some("3 items failed"));

    s.src.seed_file("/in/missing.txt", "found");
    let retry = q.retry_failed(id, Some(ConflictPolicy::Overwrite)).unwrap();
    let info = finished(&q, retry).await;
    assert_eq!(info.status, OperationStatus::Completed, "{info:?}");
    assert_eq!(info.kind, "copy");
    assert_eq!(info.failed, 0);
    assert_eq!(s.dst.inner().contents("/out/b.txt").unwrap(), b"b.txt");
    assert_eq!(
        s.dst.inner().contents("/out/missing.txt").unwrap(),
        b"found"
    );
    assert_eq!(s.dst.inner().contents("/out/sub/d.txt").unwrap(), b"d");
    assert!(
        q.retry_failed(retry, None).is_none(),
        "nothing left to retry"
    );
}

/// QUEUE-015 — an unattended move keeps the sources it could not move and
/// the default still stops at the first error.
#[tokio::test]
async fn unattended_move_keeps_failed_sources() {
    let s = setup();
    s.src.seed_file("/in/a.txt", "a");
    s.src.seed_file("/in/b.txt", "b");
    s.dst.inner().seed_file("/out/a.txt", "old");
    let q = queue(&s.registry, 2);
    let op = Operation::Move {
        sources: vec![
            Location::new("src", "/in/a.txt"),
            Location::new("src", "/in/b.txt"),
        ],
        destination: Location::new("dst", "/out"),
        conflict: ConflictPolicy::Fail,
    };
    let stop = q.enqueue(op.clone());
    let info = finished(&q, stop).await;
    assert_eq!(info.status, OperationStatus::Failed);
    assert_eq!(info.failed, 0);

    s.dst.inner().wipe();
    s.dst.inner().seed_dir("/out");
    s.dst.inner().seed_file("/out/a.txt", "old");
    let id = q.enqueue_with(
        op,
        TransferOptions {
            keep_going: true,
            ..Default::default()
        },
    );
    let info = finished(&q, id).await;
    assert_eq!(info.status, OperationStatus::Completed);
    assert_eq!(info.failed, 1);
    assert_eq!(s.src.contents("/in/a.txt").unwrap(), b"a", "kept");
    assert!(s.src.contents("/in/b.txt").is_none(), "moved");
    assert_eq!(s.dst.inner().contents("/out/b.txt").unwrap(), b"b");
    // Retrying a move moves.
    let retry = q.retry_failed(id, Some(ConflictPolicy::Overwrite)).unwrap();
    let info = finished(&q, retry).await;
    assert_eq!(
        (info.kind.as_str(), info.status),
        ("move", OperationStatus::Completed)
    );
    assert!(s.src.contents("/in/a.txt").is_none());
    assert_eq!(s.dst.inner().contents("/out/a.txt").unwrap(), b"a");
}

/// QUEUE-016 — options and the new operation kind round-trip through JSON,
/// and old JSON without options still loads.
#[test]
fn options_and_copy_items_json() {
    let opts: TransferOptions = serde_json::from_str(r#"{"keepGoing":true}"#).unwrap();
    assert!(opts.keep_going && !opts.verify && opts.limit.is_none());
    let op: Operation = serde_json::from_str(
        r#"{"type":"copyItems","items":[{"source":{"fs":"a","path":"/x"},"target":{"fs":"b","path":"/y/x"}}],"isMove":true}"#,
    )
    .unwrap();
    assert_eq!(op.kind(), "move");
    assert_eq!(op.summary(), "Move 1 item again");
    assert_eq!(op.places(), ["a", "b"]);
    let info: OperationInfo = serde_json::from_value(serde_json::json!({
        "id": uuid::Uuid::nil(), "kind": "copy", "summary": "s", "status": "paused",
        "progress": { "filesTotal": 0, "filesDone": 0, "bytesTotal": 0, "bytesDone": 0,
                      "skipped": 0, "current": null },
        "error": null, "queuedMs": 0, "startedMs": null, "finishedMs": null,
    }))
    .unwrap();
    assert_eq!(info.status, OperationStatus::Paused);
    assert!(!info.status.is_finished());
    assert!(!info.restored);
}

/// QUEUE-017 — a corrupt store file starts empty instead of failing.
#[test]
fn corrupt_store_starts_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("queue.json");
    std::fs::write(&store, "{ not json").unwrap();
    let q = OperationQueue::with_store(
        Arc::new(Registry::new()),
        Arc::new(ActivityLog::in_memory(10)),
        2,
        &store,
    );
    assert_eq!(q.restore(), 0);
    assert!(q.list().is_empty());
}

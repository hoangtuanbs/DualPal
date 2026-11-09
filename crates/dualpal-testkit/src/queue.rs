//! Operation-queue helpers shared by the core integration tests.

use std::sync::{Arc, Mutex};

use dualpal_core::{ActivityLog, Operation, OperationInfo, OperationQueue, Registry};
use uuid::Uuid;

/// A queue over `registry` with an in-memory activity log and the app's
/// usual two concurrent slots.
pub fn queue(registry: Arc<Registry>) -> OperationQueue {
    queue_with(registry, Arc::new(ActivityLog::in_memory(1000)), 2)
}

/// A queue with explicit activity log and concurrency, for tests that need
/// to see more than the default history or to force serial execution.
pub fn queue_with(
    registry: Arc<Registry>,
    activity: Arc<ActivityLog>,
    max_concurrent: usize,
) -> OperationQueue {
    OperationQueue::new(registry, activity, max_concurrent)
}

/// Enqueue `op` and wait for it to finish.
pub async fn run(q: &OperationQueue, op: Operation) -> OperationInfo {
    let id = q.enqueue(op);
    q.wait(id).await.expect("operation is known to the queue")
}

/// Records every [`OperationInfo`] broadcast by `q` until the recorder is
/// dropped. Use it to assert on the whole progress stream (OPS-COPY-002) or
/// on the order of status changes.
pub struct EventRecorder {
    events: Arc<Mutex<Vec<OperationInfo>>>,
    task: tokio::task::JoinHandle<()>,
}

impl EventRecorder {
    /// Everything recorded so far, in broadcast order.
    pub fn events(&self) -> Vec<OperationInfo> {
        self.events.lock().unwrap().clone()
    }

    /// Only the events of one operation.
    pub fn for_op(&self, id: Uuid) -> Vec<OperationInfo> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.id == id)
            .cloned()
            .collect()
    }

    /// Forget what was recorded so far, keeping the recorder running.
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl Drop for EventRecorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Start recording the queue's broadcasts. Must be called inside a Tokio
/// runtime (all integration tests are `#[tokio::test]`).
pub fn collect_events(q: &OperationQueue) -> EventRecorder {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let mut rx = q.subscribe();
    let task = tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(info) => sink.lock().unwrap().push(info),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
    EventRecorder { events, task }
}

//! Waiting for a condition without fixed sleeps.

use std::time::{Duration, Instant};

/// Poll `cond` until it is true, or panic with `what` after `timeout`.
pub async fn eventually(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    loop {
        if cond() {
            return;
        }
        if start.elapsed() >= timeout {
            panic!("timed out after {timeout:?} waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

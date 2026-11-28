//! Bandwidth limit: a token bucket that running transfers draw from.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// A token bucket holding up to one second of bytes. Transfers take what they
/// are about to send; when the bucket runs dry the debt is paid by waiting,
/// so several transfers sharing a bucket together stay under its rate.
///
/// Time comes from `tokio::time`, so tests can run it on a paused clock.
pub struct RateLimiter {
    /// Bytes per second; 0 means no limit.
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

struct Bucket {
    /// Bytes that may be sent right away; negative while in debt.
    tokens: f64,
    last: Option<Instant>,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            bucket: Mutex::new(Bucket {
                tokens: 0.0,
                last: None,
            }),
        }
    }

    /// Bytes per second, 0 when there is no limit.
    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::SeqCst)
    }

    /// Change the rate. Takes effect for the next bytes taken.
    pub fn set_rate(&self, bytes_per_sec: u64) {
        if self.rate.swap(bytes_per_sec, Ordering::SeqCst) != bytes_per_sec {
            let mut b = self.bucket.lock().unwrap();
            b.tokens = 0.0;
            b.last = None;
        }
    }

    /// Take `n` bytes from the bucket and return how long to wait before
    /// sending them (zero when there is no limit or enough is saved up).
    pub fn reserve(&self, n: u64) -> Duration {
        let rate = self.rate();
        if rate == 0 || n == 0 {
            return Duration::ZERO;
        }
        let rate = rate as f64;
        let now = Instant::now();
        let mut b = self.bucket.lock().unwrap();
        if let Some(last) = b.last {
            let refill = now.saturating_duration_since(last).as_secs_f64() * rate;
            b.tokens = (b.tokens + refill).min(rate);
        }
        b.last = Some(now);
        b.tokens -= n as f64;
        if b.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-b.tokens / rate)
        }
    }
}

/// How many bytes to read at a time under a limit of `rate` bytes per second:
/// small enough for steady progress and a quick pause, at most `max`.
pub fn chunk_for(rate: u64, max: usize) -> usize {
    if rate == 0 {
        max
    } else {
        ((rate / 8) as usize).clamp(4 * 1024, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_shrink_under_a_low_limit() {
        assert_eq!(chunk_for(0, 1_000_000), 1_000_000);
        assert_eq!(chunk_for(1024, 256 * 1024), 4 * 1024);
        assert_eq!(chunk_for(800 * 1024, 256 * 1024), 100 * 1024);
        assert_eq!(chunk_for(100 << 20, 256 * 1024), 256 * 1024);
    }

    #[tokio::test]
    async fn no_limit_never_waits() {
        let l = RateLimiter::new(0);
        assert_eq!(l.reserve(u64::MAX / 2), Duration::ZERO);
    }
}

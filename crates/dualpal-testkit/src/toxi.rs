//! Toxiproxy control for the fault-injection tests.
//!
//! Toxiproxy sits in front of FTP, WebDAV and S3 (see `docker/test-servers`).
//! Tests that cut or slow a connection must be named `fault_*` and run alone
//! (see `.config/nextest.toml`), because a toxic affects every client of that
//! server. Each call here adds a toxic that a `Drop` guard removes.

use serde_json::{json, Value};

/// The base URL from `DUALPAL_TEST_TOXIPROXY`, e.g. `http://127.0.0.1:8474`.
pub fn base() -> Option<String> {
    std::env::var("DUALPAL_TEST_TOXIPROXY")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// A handle to `proxy` (e.g. `"s3-proxy"`), `None` when toxiproxy is not set up.
pub async fn proxy(proxy: &str) -> Option<ToxicGuard> {
    let base = base()?;
    let url = format!("{base}/proxies/{proxy}/toxics");
    let client = reqwest::Client::new();
    Some(ToxicGuard {
        client,
        url,
        name: format!("dualpal-{}", uuid::Uuid::new_v4().simple()),
    })
}

/// Removes the toxic when dropped.
pub struct ToxicGuard {
    client: reqwest::Client,
    url: String,
    name: String,
}

impl ToxicGuard {
    async fn add(&self, toxic_type: &str, attributes: Value) {
        let body = serde_json::to_vec(&json!({
            "name": self.name,
            "type": toxic_type,
            "stream": "downstream",
            "attributes": attributes,
        }))
        .unwrap();
        let _ = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await;
    }

    /// Add a latency of `ms` milliseconds to every connection.
    pub async fn latency(&self, ms: u64) {
        self.add("latency", json!({ "latency": ms, "jitter": 0 }))
            .await;
    }

    /// Limit the connection to `kbps` kilobits per second.
    pub async fn bandwidth(&self, kbps: u64) {
        self.add("bandwidth", json!({ "rate": kbps })).await;
    }

    /// Close the connection after `bytes` bytes have flowed.
    pub async fn limit_data(&self, bytes: u64) {
        self.add("limit_data", json!({ "bytes": bytes })).await;
    }
}

impl Drop for ToxicGuard {
    fn drop(&mut self) {
        let url = format!("{}/{}", self.url, self.name);
        let client = self.client.clone();
        // Best effort: the test is over either way.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = client.delete(&url).send().await;
            });
        }
    }
}

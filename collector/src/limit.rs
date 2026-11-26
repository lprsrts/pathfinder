//! Limits on requests: how many run at once, and how often one address may
//! call the endpoints anyone can reach. Addresses stay in memory for a
//! minute and are never written down.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

const WINDOW: Duration = Duration::from_secs(60);

/// Past this many tracked addresses, old windows are dropped.
const MAX_TRACKED: usize = 10_000;

/// Fixed one-minute windows per (address, bucket).
#[derive(Default)]
pub struct RateLimiter {
    /// (address, bucket) → (window start, requests in it).
    windows: Mutex<HashMap<Key, (Instant, u32)>>,
}

type Key = (String, &'static str);

impl RateLimiter {
    /// Counts one request from `addr` in `bucket`; false past `per_minute`.
    pub fn allow(&self, addr: &str, bucket: &'static str, per_minute: u32) -> bool {
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap();
        if windows.len() > MAX_TRACKED {
            windows.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
        }
        let entry = windows
            .entry((addr.to_string(), bucket))
            .or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= per_minute
    }
}

/// The caller's address as Cloudflare saw it; requests that didn't come
/// through Cloudflare (local runs) share one bucket.
pub fn client_addr(headers: &HeaderMap) -> String {
    headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("direct")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_address_and_bucket_has_its_own_window() {
        let limiter = RateLimiter::default();
        assert!(limiter.allow("a", "link", 2));
        assert!(limiter.allow("a", "link", 2));
        assert!(!limiter.allow("a", "link", 2));
        assert!(limiter.allow("b", "link", 2));
        assert!(limiter.allow("a", "poll", 2));
    }
}

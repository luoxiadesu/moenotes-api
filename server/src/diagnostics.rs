use serde::Serialize;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
pub struct Diagnostics {
    start: Instant,
    requests: AtomicU64,
    errors: AtomicU64,
    inflight: AtomicU64,
}
#[derive(Serialize)]
pub struct Snapshot {
    uptime_seconds: u64,
    requests: u64,
    errors: u64,
    inflight: u64,
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            requests: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
        }
    }
}
pub struct Guard<'a>(&'a Diagnostics);
impl Diagnostics {
    pub fn begin(&self) -> Guard<'_> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.inflight.fetch_add(1, Ordering::Relaxed);
        Guard(self)
    }
    pub fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            uptime_seconds: self.start.elapsed().as_secs(),
            requests: self.requests.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            inflight: self.inflight.load(Ordering::Relaxed),
        }
    }
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

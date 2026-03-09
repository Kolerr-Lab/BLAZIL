//! Engine performance metrics.
//!
//! All counters use atomic operations — no locks, no contention on the hot
//! path. The metrics object is shared via `Arc<EngineMetrics>` and can be
//! read from any thread without blocking.
//!
//! # Examples
//!
//! ```rust
//! use blazil_engine::metrics::EngineMetrics;
//!
//! let m = EngineMetrics::new();
//! m.record_published();
//! m.record_committed(1_500);
//! m.record_rejected();
//! assert_eq!(m.published(), 1);
//! assert_eq!(m.committed(), 1);
//! assert_eq!(m.rejected(), 1);
//! assert_eq!(m.avg_latency_ns(), 1_500);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ── EngineMetrics ─────────────────────────────────────────────────────────────

/// Lock-free performance counters for the Blazil engine.
///
/// Use [`EngineMetrics::new`] to obtain an `Arc<Self>` that can be shared
/// across handler threads and the monitoring subsystem.
pub struct EngineMetrics {
    events_published: AtomicU64,
    events_committed: AtomicU64,
    events_rejected: AtomicU64,
    /// Running sum of latency in nanoseconds.
    /// Divide by `events_committed` to get the average.
    total_latency_ns: AtomicU64,
}

impl EngineMetrics {
    /// Creates a new `Arc<EngineMetrics>` with all counters at zero.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// assert_eq!(m.published(), 0);
    /// ```
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events_published: AtomicU64::new(0),
            events_committed: AtomicU64::new(0),
            events_rejected: AtomicU64::new(0),
            total_latency_ns: AtomicU64::new(0),
        })
    }

    /// Increments the published counter.
    ///
    /// Call this when a new event enters the pipeline.
    #[inline]
    pub fn record_published(&self) {
        self.events_published.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the committed counter and adds `latency_ns` to the total.
    ///
    /// `latency_ns` is the nanoseconds elapsed from ingestion to ledger commit.
    #[inline]
    pub fn record_committed(&self, latency_ns: u64) {
        self.events_committed.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ns
            .fetch_add(latency_ns, Ordering::Relaxed);
    }

    /// Increments the rejected counter.
    ///
    /// Call this when an event is rejected by any handler.
    #[inline]
    pub fn record_rejected(&self) {
        self.events_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the number of events published.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// m.record_published();
    /// assert_eq!(m.published(), 1);
    /// ```
    #[inline]
    pub fn published(&self) -> u64 {
        self.events_published.load(Ordering::Relaxed)
    }

    /// Returns the number of events committed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// m.record_committed(500);
    /// assert_eq!(m.committed(), 1);
    /// ```
    #[inline]
    pub fn committed(&self) -> u64 {
        self.events_committed.load(Ordering::Relaxed)
    }

    /// Returns the number of events rejected.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// m.record_rejected();
    /// assert_eq!(m.rejected(), 1);
    /// ```
    #[inline]
    pub fn rejected(&self) -> u64 {
        self.events_rejected.load(Ordering::Relaxed)
    }

    /// Returns the average commit latency in nanoseconds.
    ///
    /// Returns `0` if no events have been committed yet.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// m.record_committed(1000);
    /// m.record_committed(2000);
    /// assert_eq!(m.avg_latency_ns(), 1500);
    /// ```
    #[inline]
    pub fn avg_latency_ns(&self) -> u64 {
        let committed = self.events_committed.load(Ordering::Relaxed);
        if committed == 0 {
            return 0;
        }
        self.total_latency_ns.load(Ordering::Relaxed) / committed
    }

    /// Prints a human-readable summary to stdout.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_engine::metrics::EngineMetrics;
    ///
    /// let m = EngineMetrics::new();
    /// m.print_summary(); // outputs zeroed counters
    /// ```
    pub fn print_summary(&self) {
        println!("Events published:  {}", self.published());
        println!("Events committed:  {}", self.committed());
        println!("Events rejected:   {}", self.rejected());
        println!("Avg latency:       {} ns", self.avg_latency_ns());
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_published_increments_counter() {
        let m = EngineMetrics::new();
        m.record_published();
        m.record_published();
        assert_eq!(m.published(), 2);
    }

    #[test]
    fn record_committed_increments_counter() {
        let m = EngineMetrics::new();
        m.record_committed(1_000);
        assert_eq!(m.committed(), 1);
    }

    #[test]
    fn record_committed_accumulates_latency() {
        let m = EngineMetrics::new();
        m.record_committed(1_000);
        m.record_committed(3_000);
        assert_eq!(m.avg_latency_ns(), 2_000);
    }

    #[test]
    fn record_rejected_increments_counter() {
        let m = EngineMetrics::new();
        m.record_rejected();
        m.record_rejected();
        m.record_rejected();
        assert_eq!(m.rejected(), 3);
    }

    #[test]
    fn avg_latency_returns_zero_when_no_commits() {
        let m = EngineMetrics::new();
        assert_eq!(m.avg_latency_ns(), 0);
    }

    #[test]
    fn avg_latency_correct_with_multiple_samples() {
        let m = EngineMetrics::new();
        for latency in [500_u64, 1_000, 1_500, 2_000, 2_500] {
            m.record_committed(latency);
        }
        // sum = 7500, count = 5 → avg = 1500
        assert_eq!(m.avg_latency_ns(), 1_500);
    }
}

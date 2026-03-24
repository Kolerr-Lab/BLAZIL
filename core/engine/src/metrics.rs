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
    /// Peak observed commit latency in nanoseconds (proxy for p99).
    peak_ns: AtomicU64,
    /// Ring buffer utilization × 10_000 (i.e. 10000 = 100%).
    ring_util_x10000: AtomicU64,
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
            peak_ns: AtomicU64::new(0),
            ring_util_x10000: AtomicU64::new(0),
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
        // Update peak (proxy for p99) using compare-and-swap loop.
        let mut current = self.peak_ns.load(Ordering::Relaxed);
        while latency_ns > current {
            match self.peak_ns.compare_exchange_weak(
                current,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
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
        println!("Peak latency:      {} ns", self.peak_latency_ns());
    }

    /// Returns the peak (maximum) observed commit latency in nanoseconds.
    /// Used as a proxy for p99 without requiring a histogram.
    #[inline]
    pub fn peak_latency_ns(&self) -> u64 {
        self.peak_ns.load(Ordering::Relaxed)
    }

    /// Returns the ring buffer utilization × 10_000 (10_000 = 100%).
    #[inline]
    pub fn ring_utilization_x10000(&self) -> u64 {
        self.ring_util_x10000.load(Ordering::Relaxed)
    }

    /// Sets ring buffer utilization; value is utilization × 10_000.
    pub fn set_ring_utilization_x10000(&self, val: u64) {
        self.ring_util_x10000.store(val, Ordering::Relaxed);
    }
}

// ── ShardMetrics ─────────────────────────────────────────────────────────────

/// Per-shard lock-free performance counters.
///
/// One instance per shard, shared via `Arc<ShardMetrics>`. Designed to be
/// updated on the hot path without any locking.
///
/// # Metric names (Prometheus convention)
///
/// | Metric | Kind | Description |
/// |---|---|---|
/// | `blazil_shard_transactions_total{shard="N"}` | counter | Events processed |
/// | `blazil_shard_latency_p99_ns{shard="N"}` | gauge | Peak (proxy p99) latency |
/// | `blazil_shard_ring_buffer_utilization{shard="N"}` | gauge | 0–10000 (10000=100%) |
/// | `blazil_shard_backpressure_total{shard="N"}` | counter | Ring-full events |
pub struct ShardMetrics {
    /// Shard index for labelling.
    pub shard_id: usize,
    /// `blazil_shard_transactions_total`
    transactions_total: AtomicU64,
    /// `blazil_shard_latency_p99_ns` — peak latency as p99 proxy.
    latency_peak_ns: AtomicU64,
    /// `blazil_shard_ring_buffer_utilization` × 10_000.
    ring_util_x10000: AtomicU64,
    /// `blazil_shard_backpressure_total`
    backpressure_total: AtomicU64,
}

impl ShardMetrics {
    /// Create a new `Arc<ShardMetrics>` for `shard_id` with all counters zeroed.
    pub fn new(shard_id: usize) -> Arc<Self> {
        Arc::new(Self {
            shard_id,
            transactions_total: AtomicU64::new(0),
            latency_peak_ns: AtomicU64::new(0),
            ring_util_x10000: AtomicU64::new(0),
            backpressure_total: AtomicU64::new(0),
        })
    }

    /// Increment `blazil_shard_transactions_total` and update peak latency.
    #[inline]
    pub fn record_transaction(&self, latency_ns: u64) {
        self.transactions_total.fetch_add(1, Ordering::Relaxed);
        let mut current = self.latency_peak_ns.load(Ordering::Relaxed);
        while latency_ns > current {
            match self.latency_peak_ns.compare_exchange_weak(
                current,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Update `blazil_shard_ring_buffer_utilization`.
    ///
    /// `util_x10000` is utilization × 10_000 (10_000 = 100%).
    #[inline]
    pub fn set_ring_utilization(&self, util_x10000: u64) {
        self.ring_util_x10000.store(util_x10000, Ordering::Relaxed);
    }

    /// Increment `blazil_shard_backpressure_total`.
    #[inline]
    pub fn record_backpressure(&self) {
        self.backpressure_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns total transactions processed by this shard.
    #[inline]
    pub fn transactions_total(&self) -> u64 {
        self.transactions_total.load(Ordering::Relaxed)
    }

    /// Returns the peak observed latency (proxy for p99) in nanoseconds.
    #[inline]
    pub fn latency_p99_ns(&self) -> u64 {
        self.latency_peak_ns.load(Ordering::Relaxed)
    }

    /// Returns ring buffer utilization × 10_000 (10_000 = 100%).
    #[inline]
    pub fn ring_utilization_x10000(&self) -> u64 {
        self.ring_util_x10000.load(Ordering::Relaxed)
    }

    /// Returns total backpressure events (ring-full rejections).
    #[inline]
    pub fn backpressure_total(&self) -> u64 {
        self.backpressure_total.load(Ordering::Relaxed)
    }

    /// Returns ring buffer utilization as a float in [0.0, 1.0].
    #[inline]
    pub fn ring_utilization_f64(&self) -> f64 {
        self.ring_util_x10000.load(Ordering::Relaxed) as f64 / 10_000.0
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

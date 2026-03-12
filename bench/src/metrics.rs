//! Benchmark result storage and statistics.

use std::time::Duration;

/// Results from a single benchmark scenario.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub scenario:     String,
    pub total_events: u64,
    pub duration_ms:  u64,
    pub tps:          u64,   // transactions per second
    pub mean_ns:      u64,   // mean latency per tx
    pub min_ns:       u64,
    pub max_ns:       u64,
    pub p50_ns:       u64,
    pub p95_ns:       u64,
    pub p99_ns:       u64,
    pub p99_9_ns:     u64,
}

impl BenchmarkResult {
    /// Build a result from raw measurements.
    ///
    /// `latencies` is **sorted in place** as a side-effect.
    /// All percentile indices use integer division to stay reproducible.
    pub fn new(
        scenario: &str,
        events: u64,
        duration: Duration,
        latencies: &mut [u64],
    ) -> Self {
        latencies.sort_unstable();

        let len = latencies.len();
        let duration_ms = duration.as_millis().max(1) as u64;
        let tps = events * 1_000 / duration_ms;

        let mean_ns = if len > 0 {
            (latencies.iter().map(|&x| x as u128).sum::<u128>() / len as u128) as u64
        } else {
            0
        };

        let min_ns    = latencies.first().copied().unwrap_or(0);
        let max_ns    = latencies.last().copied().unwrap_or(0);
        let p50_ns    = percentile(latencies, 50);
        let p95_ns    = percentile(latencies, 95);
        let p99_ns    = percentile(latencies, 99);
        let p99_9_ns  = percentile_thousandths(latencies, 999);

        Self {
            scenario: scenario.to_owned(),
            total_events: events,
            duration_ms,
            tps,
            mean_ns,
            min_ns,
            max_ns,
            p50_ns,
            p95_ns,
            p99_ns,
            p99_9_ns,
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Returns the value at percentile `p` (0–99) of a **sorted** slice.
fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (sorted.len() * p / 100).min(sorted.len() - 1);
    sorted[idx]
}

/// Returns the value at per-mille `p` (0–999) of a **sorted** slice.
fn percentile_thousandths(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (sorted.len() * p / 1_000).min(sorted.len() - 1);
    sorted[idx]
}

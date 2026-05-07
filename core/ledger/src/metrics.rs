//! Metrics instrumentation for ledger operations.
//!
//! Provides counters and histograms for monitoring ledger health in production.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Ledger metrics collected during runtime.
///
/// All operations are atomic and lock-free for minimal performance impact.
#[derive(Debug, Clone)]
pub struct LedgerMetrics {
    inner: Arc<LedgerMetricsInner>,
}

#[derive(Debug)]
struct LedgerMetricsInner {
    // Account operations
    accounts_created_total: AtomicU64,
    accounts_created_errors: AtomicU64,
    account_lookups_total: AtomicU64,
    account_lookups_errors: AtomicU64,

    // Transfer operations
    transfers_created_total: AtomicU64,
    transfers_created_errors: AtomicU64,
    transfers_batch_total: AtomicU64,
    transfers_batch_partial_failures: AtomicU64,
    transfers_batch_transport_errors: AtomicU64,
    transfer_lookups_total: AtomicU64,
    transfer_lookups_errors: AtomicU64,

    // Batch operations
    batch_account_lookups_total: AtomicU64,
    batch_account_lookups_errors: AtomicU64,
}

impl LedgerMetrics {
    /// Creates a new metrics collector with all counters at zero.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(LedgerMetricsInner {
                accounts_created_total: AtomicU64::new(0),
                accounts_created_errors: AtomicU64::new(0),
                account_lookups_total: AtomicU64::new(0),
                account_lookups_errors: AtomicU64::new(0),
                transfers_created_total: AtomicU64::new(0),
                transfers_created_errors: AtomicU64::new(0),
                transfers_batch_total: AtomicU64::new(0),
                transfers_batch_partial_failures: AtomicU64::new(0),
                transfers_batch_transport_errors: AtomicU64::new(0),
                transfer_lookups_total: AtomicU64::new(0),
                transfer_lookups_errors: AtomicU64::new(0),
                batch_account_lookups_total: AtomicU64::new(0),
                batch_account_lookups_errors: AtomicU64::new(0),
            }),
        }
    }

    // Account metrics
    pub fn inc_accounts_created(&self) {
        self.inner
            .accounts_created_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_accounts_created_errors(&self) {
        self.inner
            .accounts_created_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_account_lookups(&self) {
        self.inner
            .account_lookups_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_account_lookups_errors(&self) {
        self.inner
            .account_lookups_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    // Transfer metrics
    pub fn inc_transfers_created(&self) {
        self.inner
            .transfers_created_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transfers_created_errors(&self) {
        self.inner
            .transfers_created_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transfers_batch(&self, count: u64) {
        self.inner
            .transfers_batch_total
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_transfers_batch_partial_failures(&self) {
        self.inner
            .transfers_batch_partial_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transfers_batch_transport_errors(&self) {
        self.inner
            .transfers_batch_transport_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transfer_lookups(&self) {
        self.inner
            .transfer_lookups_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transfer_lookups_errors(&self) {
        self.inner
            .transfer_lookups_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    // Batch metrics
    pub fn inc_batch_account_lookups(&self, count: u64) {
        self.inner
            .batch_account_lookups_total
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn inc_batch_account_lookups_errors(&self) {
        self.inner
            .batch_account_lookups_errors
            .fetch_add(1, Ordering::Relaxed);
    }

    // Getters for monitoring/export
    pub fn accounts_created_total(&self) -> u64 {
        self.inner.accounts_created_total.load(Ordering::Relaxed)
    }

    pub fn accounts_created_errors(&self) -> u64 {
        self.inner.accounts_created_errors.load(Ordering::Relaxed)
    }

    pub fn account_lookups_total(&self) -> u64 {
        self.inner.account_lookups_total.load(Ordering::Relaxed)
    }

    pub fn account_lookups_errors(&self) -> u64 {
        self.inner.account_lookups_errors.load(Ordering::Relaxed)
    }

    pub fn transfers_created_total(&self) -> u64 {
        self.inner.transfers_created_total.load(Ordering::Relaxed)
    }

    pub fn transfers_created_errors(&self) -> u64 {
        self.inner.transfers_created_errors.load(Ordering::Relaxed)
    }

    pub fn transfers_batch_total(&self) -> u64 {
        self.inner.transfers_batch_total.load(Ordering::Relaxed)
    }

    pub fn transfers_batch_partial_failures(&self) -> u64 {
        self.inner
            .transfers_batch_partial_failures
            .load(Ordering::Relaxed)
    }

    pub fn transfers_batch_transport_errors(&self) -> u64 {
        self.inner
            .transfers_batch_transport_errors
            .load(Ordering::Relaxed)
    }

    pub fn transfer_lookups_total(&self) -> u64 {
        self.inner.transfer_lookups_total.load(Ordering::Relaxed)
    }

    pub fn transfer_lookups_errors(&self) -> u64 {
        self.inner.transfer_lookups_errors.load(Ordering::Relaxed)
    }

    pub fn batch_account_lookups_total(&self) -> u64 {
        self.inner
            .batch_account_lookups_total
            .load(Ordering::Relaxed)
    }

    pub fn batch_account_lookups_errors(&self) -> u64 {
        self.inner
            .batch_account_lookups_errors
            .load(Ordering::Relaxed)
    }

    /// Returns a snapshot of all metrics as key-value pairs.
    ///
    /// Useful for Prometheus export or logging.
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("accounts_created_total", self.accounts_created_total()),
            ("accounts_created_errors", self.accounts_created_errors()),
            ("account_lookups_total", self.account_lookups_total()),
            ("account_lookups_errors", self.account_lookups_errors()),
            ("transfers_created_total", self.transfers_created_total()),
            ("transfers_created_errors", self.transfers_created_errors()),
            ("transfers_batch_total", self.transfers_batch_total()),
            (
                "transfers_batch_partial_failures",
                self.transfers_batch_partial_failures(),
            ),
            (
                "transfers_batch_transport_errors",
                self.transfers_batch_transport_errors(),
            ),
            ("transfer_lookups_total", self.transfer_lookups_total()),
            ("transfer_lookups_errors", self.transfer_lookups_errors()),
            (
                "batch_account_lookups_total",
                self.batch_account_lookups_total(),
            ),
            (
                "batch_account_lookups_errors",
                self.batch_account_lookups_errors(),
            ),
        ]
    }
}

impl Default for LedgerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metrics_start_at_zero() {
        let metrics = LedgerMetrics::new();
        assert_eq!(metrics.accounts_created_total(), 0);
        assert_eq!(metrics.transfers_created_total(), 0);
    }

    #[test]
    fn inc_accounts_created_increments() {
        let metrics = LedgerMetrics::new();
        metrics.inc_accounts_created();
        metrics.inc_accounts_created();
        assert_eq!(metrics.accounts_created_total(), 2);
    }

    #[test]
    fn inc_transfers_batch_adds_count() {
        let metrics = LedgerMetrics::new();
        metrics.inc_transfers_batch(10);
        metrics.inc_transfers_batch(5);
        assert_eq!(metrics.transfers_batch_total(), 15);
    }

    #[test]
    fn metrics_are_independent() {
        let metrics = LedgerMetrics::new();
        metrics.inc_accounts_created();
        metrics.inc_transfers_created();
        assert_eq!(metrics.accounts_created_total(), 1);
        assert_eq!(metrics.transfers_created_total(), 1);
        assert_eq!(metrics.account_lookups_total(), 0);
    }

    #[test]
    fn snapshot_includes_all_metrics() {
        let metrics = LedgerMetrics::new();
        metrics.inc_accounts_created();
        metrics.inc_transfers_batch(5);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.len(), 13);

        let accounts_created = snapshot
            .iter()
            .find(|(k, _)| *k == "accounts_created_total");
        assert_eq!(accounts_created.map(|(_, v)| *v), Some(1));

        let batch_total = snapshot.iter().find(|(k, _)| *k == "transfers_batch_total");
        assert_eq!(batch_total.map(|(_, v)| *v), Some(5));
    }

    #[test]
    fn clone_shares_state() {
        let metrics1 = LedgerMetrics::new();
        let metrics2 = metrics1.clone();

        metrics1.inc_accounts_created();
        assert_eq!(metrics2.accounts_created_total(), 1);

        metrics2.inc_accounts_created();
        assert_eq!(metrics1.accounts_created_total(), 2);
    }
}

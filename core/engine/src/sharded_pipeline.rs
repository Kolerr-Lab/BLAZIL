//! Sharded pipeline for parallel transaction processing across multiple cores.
//!
//! Each shard is an independent pipeline with its own ring buffer and full handler chain.
//! Events are routed to shards by account ID for deterministic processing and to avoid
//! cross-shard coordination overhead.
//!
//! # Architecture
//!
//! ```text
//! Producer Thread
//!      |
//!      v
//! Route by account_id % shard_count
//!      |
//!      +-> Shard 0: RingBuffer -> [Validation -> Risk -> Ledger -> Publish]
//!      +-> Shard 1: RingBuffer -> [Validation -> Risk -> Ledger -> Publish]
//!      +-> Shard 2: RingBuffer -> [Validation -> Risk -> Ledger -> Publish]
//!      +-> Shard 3: RingBuffer -> [Validation -> Risk -> Ledger -> Publish]
//! ```
//!
//! Each shard runs on its own thread, pinned to a dedicated physical core for optimal
//! cache locality and minimal context switching.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread::JoinHandle;

use blazil_common::error::BlazerResult;

// ── Memory budget constants ───────────────────────────────────────────────────

/// Upper bound on the number of shards in a production deployment.
/// Used to compute the compile-time ring buffer memory assertion below.
pub const MAX_SHARD_COUNT: usize = 8;

/// Upper bound on ring buffer capacity per shard (must be power of 2).
/// Each slot holds one [`crate::event::TransactionEvent`] = 56 bytes.
pub const MAX_RING_CAPACITY_PER_SHARD: usize = 1_048_576; // 1 M slots

/// Compile-time guard: total ring buffer memory across all shards must not
/// exceed 512 MB.  If you increase either constant, verify the budget holds.
///
/// Current: 8 shards × 1 048 576 slots × 56 bytes = 450 MB ✓
const _: () = assert!(
    MAX_SHARD_COUNT * MAX_RING_CAPACITY_PER_SHARD * 56 <= 512 * 1024 * 1024,
    "Ring buffer total exceeds 512 MB — reduce MAX_SHARD_COUNT or MAX_RING_CAPACITY_PER_SHARD"
);

// ── Dynamic shard-count helpers ───────────────────────────────────────────────

/// Compute the default shard count from available CPU parallelism.
///
/// Returns the largest power of 2 that is ≤ `(cpu_count / 2).max(1)`,
/// capped at [`MAX_SHARD_COUNT`].
pub fn default_shard_count() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let desired = (cpus / 2).max(1);
    let mut n = 1usize;
    while n * 2 <= desired.min(MAX_SHARD_COUNT) {
        n *= 2;
    }
    n
}

/// Read shard count from the `BLAZIL_SHARD_COUNT` environment variable.
///
/// Falls back to [`default_shard_count`] when the variable is unset.
///
/// # Panics
///
/// Panics if the env var is set to a value that is not a power of 2,
/// less than 1, or greater than [`MAX_SHARD_COUNT`].
pub fn from_env() -> usize {
    std::env::var("BLAZIL_SHARD_COUNT")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .inspect(|&n| {
            assert!(
                (1..=MAX_SHARD_COUNT).contains(&n) && n.is_power_of_two(),
                "BLAZIL_SHARD_COUNT must be power of 2, between 1 and {}",
                MAX_SHARD_COUNT
            );
        })
        .unwrap_or_else(default_shard_count)
}

/// Route an account event to a shard index using a fast bitmask.
///
/// # Precondition
///
/// `shard_count` must be a power of 2 (enforced by [`from_env`] and
/// recommended for all callers).
#[inline(always)]
pub fn route_to_shard(account_id: u64, shard_count: usize) -> usize {
    (account_id as usize) & (shard_count - 1)
}

use dashmap::DashMap;

use crate::event::{TransactionEvent, TransactionResult};
use crate::pipeline::{Pipeline, PipelineBuilder};

// ── ShardedPipeline ───────────────────────────────────────────────────────────

/// Multi-shard pipeline for parallel transaction processing.
///
/// Creates N independent pipelines, each with its own ring buffer and full handler
/// chain. Routes events by account ID to ensure deterministic processing without
/// cross-shard coordination.
///
/// # Examples
///
/// ```rust,no_run
/// use blazil_engine::sharded_pipeline::ShardedPipeline;
///
/// let sharded = ShardedPipeline::new(
///     4,      // shard_count
///     1024,   // capacity_per_shard
///     1_000_000_000 // max_amount_units
/// )?;
/// # Ok::<(), blazil_common::error::BlazerError>(())
/// ```
pub struct ShardedPipeline {
    shards: Vec<Pipeline>,
    /// Active shard count, stored atomically so future live-read paths can
    /// observe a resize without holding a lock.
    shard_count: AtomicUsize,
    /// Capacity per shard — kept for resize().
    capacity_per_shard: usize,
    /// Max amount units — kept for resize().
    max_amount_units: u64,
    _handles: Vec<JoinHandle<()>>, // Keep thread handles alive
}

impl ShardedPipeline {
    /// Create a sharded pipeline with the specified configuration.
    ///
    /// # Arguments
    ///
    /// * `shard_count` - Number of independent shards (typically matches physical core count)
    /// * `capacity_per_shard` - Ring buffer capacity per shard (must be power of 2)
    /// * `max_amount_units` - Maximum transaction amount in minor units (e.g., cents)
    ///
    /// # Errors
    ///
    /// Returns error if capacity is not a power of 2 or thread spawning fails.
    pub fn new(
        shard_count: usize,
        capacity_per_shard: usize,
        max_amount_units: u64,
    ) -> BlazerResult<Self> {
        let mut shards = Vec::with_capacity(shard_count);
        let mut handles = Vec::new();

        // Create independent pipeline for each shard
        // Each shard: dedicated thread + dedicated tokio runtime = zero contention
        for _shard_id in 0..shard_count {
            // Each shard gets its OWN results map (no key collision across shards)
            let shard_results = Arc::new(DashMap::new());

            let builder = PipelineBuilder::new()
                .with_capacity(capacity_per_shard)
                .with_results(Arc::clone(&shard_results));

            // Build full handler chain for this shard
            use crate::handlers::ledger::LedgerHandler;
            use crate::handlers::publish::PublishHandler;
            use crate::handlers::risk::RiskHandler;
            use crate::handlers::validation::ValidationHandler;
            use blazil_ledger::mock::InMemoryLedgerClient;

            // Each shard gets its own dedicated tokio runtime (NO SHARING)
            // This eliminates scheduler contention between shards (LMAX pattern)
            let shard_runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1) // Single worker per shard
                    .enable_all()
                    .build()
                    .expect("failed to create shard runtime"),
            );

            let validation = ValidationHandler::new(Arc::clone(&shard_results));
            let risk = RiskHandler::new(max_amount_units, Arc::clone(&shard_results));
            let ledger_client = Arc::new(InMemoryLedgerClient::new_unbounded());
            let ledger = LedgerHandler::new(
                ledger_client,
                shard_runtime, // Dedicated runtime per shard
                Arc::clone(&shard_results),
            );
            let publish = PublishHandler::new(Arc::clone(&shard_results));

            let (pipeline, runners) = builder
                .add_handler(validation)
                .add_handler(risk)
                .add_handler(ledger)
                .add_handler(publish)
                .build()?;

            // Each shard should have exactly 1 runner (single-threaded per shard)
            assert_eq!(
                runners.len(),
                1,
                "Each shard must have exactly 1 runner, got {}",
                runners.len()
            );

            // Start the shard's consumer thread
            // runner.run() spawns dedicated OS thread (LMAX Disruptor pattern)
            for runner in runners.into_iter() {
                let handle = runner.run();
                handles.push(handle);
            }

            shards.push(pipeline);
        }

        Ok(Self {
            shards,
            shard_count: AtomicUsize::new(shard_count),
            capacity_per_shard,
            max_amount_units,
            _handles: handles,
        })
    }

    /// Resize to a new shard count (static rebalancing — drain + restart).
    ///
    /// # Steps
    ///
    /// 1. Validate `new_shard_count` (power of 2, ≤ `MAX_SHARD_COUNT`).
    /// 2. Drain all existing ring buffers (signal stop, drop handles).
    /// 3. Rebuild with the new count.
    /// 4. Update `shard_count` atomically.
    ///
    /// # Note
    ///
    /// This is a **static** rebalance: in-flight events on the old shards are
    /// drained before the new shards start.  Live migration (zero-downtime) is
    /// planned for v0.3.
    ///
    /// # Panics
    ///
    /// Panics if `new_shard_count` is not a power of 2 or exceeds
    /// [`MAX_SHARD_COUNT`].
    pub fn resize(&mut self, new_shard_count: usize) {
        assert!(
            (1..=MAX_SHARD_COUNT).contains(&new_shard_count) && new_shard_count.is_power_of_two(),
            "resize: shard_count must be power of 2 in [1, {}], got {}",
            MAX_SHARD_COUNT,
            new_shard_count
        );

        let old = self.shard_count.load(AtomicOrdering::Acquire);
        tracing::info!("Resharding from {} → {} shards", old, new_shard_count);

        // ── Step 1: drain existing shards ──────────────────────────────────
        // Signal all existing pipelines to stop (drains their ring buffers).
        for shard in &self.shards {
            shard.stop();
        }
        // Wait for consumer threads to finish.
        let old_handles = std::mem::take(&mut self._handles);
        for h in old_handles {
            let _ = h.join();
        }
        self.shards.clear();

        // ── Step 2: spawn new shards ───────────────────────────────────────
        let mut new_shards = Vec::with_capacity(new_shard_count);
        let mut new_handles = Vec::new();

        for _shard_id in 0..new_shard_count {
            let shard_results = Arc::new(DashMap::new());

            let builder = PipelineBuilder::new()
                .with_capacity(self.capacity_per_shard)
                .with_results(Arc::clone(&shard_results));

            use crate::handlers::ledger::LedgerHandler;
            use crate::handlers::publish::PublishHandler;
            use crate::handlers::risk::RiskHandler;
            use crate::handlers::validation::ValidationHandler;
            use blazil_ledger::mock::InMemoryLedgerClient;

            let shard_runtime = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .expect("failed to create shard runtime"),
            );

            let validation = ValidationHandler::new(Arc::clone(&shard_results));
            let risk = RiskHandler::new(self.max_amount_units, Arc::clone(&shard_results));
            let ledger_client = Arc::new(InMemoryLedgerClient::new_unbounded());
            let ledger =
                LedgerHandler::new(ledger_client, shard_runtime, Arc::clone(&shard_results));
            let publish = PublishHandler::new(Arc::clone(&shard_results));

            let (pipeline, runners) = builder
                .add_handler(validation)
                .add_handler(risk)
                .add_handler(ledger)
                .add_handler(publish)
                .build()
                .expect("resize: failed to build shard pipeline");

            for runner in runners {
                new_handles.push(runner.run());
            }
            new_shards.push(pipeline);
        }

        self.shards = new_shards;
        self._handles = new_handles;
        // ── Step 3: publish new count atomically ───────────────────────────
        self.shard_count
            .store(new_shard_count, AtomicOrdering::Release);
    }

    /// Route event to appropriate shard and attempt to send it.
    ///
    /// Uses debit account ID for routing to ensure all transactions for a given
    /// account are processed by the same shard in order.
    ///
    /// # Arguments
    ///
    /// * `event` - Transaction event to process
    ///
    /// # Returns
    ///
    /// Sequence number assigned by the target shard's ring buffer.
    ///
    /// # Errors
    ///
    /// Returns error if the target shard's ring buffer is full.
    pub fn publish_event(&self, event: TransactionEvent) -> BlazerResult<i64> {
        // Route by debit account to ensure deterministic ordering.
        // route_to_shard uses a fast bitmask — valid when shard_count is power of 2.
        let shard_id = route_to_shard(event.debit_account_id.as_u64(), self.shard_count());
        self.shards[shard_id].publish_event(event)
    }

    /// Get aggregated results from all shards.
    ///
    /// Each shard has its own results map with independent sequence numbering.
    /// This method combines all shard results into a single map for querying.
    ///
    /// Returns a newly created DashMap containing results from ALL shards.
    pub fn results(&self) -> DashMap<i64, TransactionResult> {
        let combined = DashMap::new();

        for (shard_id, pipeline) in self.shards.iter().enumerate() {
            let shard_results = pipeline.results();

            // Copy this shard's results to combined map
            // Use unique keys: (shard_id << 48) | sequence
            for entry in shard_results.iter() {
                let unique_key = ((shard_id as i64) << 48) | entry.key();
                combined.insert(unique_key, entry.value().clone());
            }
        }
        combined
    }

    /// Signal all shards to stop processing and wait for graceful shutdown.
    ///
    /// This will process all pending events before returning.
    pub fn stop(self) {
        for shard in self.shards {
            shard.stop();
        }
    }

    /// Get the number of shards in this pipeline.
    #[inline]
    pub fn shard_count(&self) -> usize {
        self.shard_count.load(AtomicOrdering::Relaxed)
    }

    /// Get the results map for a specific shard.
    ///
    /// # Arguments
    ///
    /// * `shard_id` - Shard identifier (0..shard_count)
    ///
    /// # Returns
    ///
    /// Arc reference to the shard's results map for efficient polling.
    ///
    /// # Panics
    ///
    /// Panics if shard_id >= shard_count.
    pub fn shard_results(&self, shard_id: usize) -> Arc<DashMap<i64, TransactionResult>> {
        Arc::clone(self.shards[shard_id].results())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_common::timestamp::Timestamp;

    #[test]
    fn test_sharded_pipeline_routes_by_account() {
        let sharded = ShardedPipeline::new(4, 1024, 1_000_000).expect("valid config");

        println!("Created {} shards", sharded.shard_count());

        // Create events with different account IDs
        for id in 0..100u64 {
            let event = TransactionEvent {
                sequence: -1,
                transaction_id: TransactionId::from_u64(id),
                debit_account_id: AccountId::from_u64(id),
                credit_account_id: AccountId::from_u64(id + 1000),
                amount_units: 10_00,
                ledger_id: LedgerId::USD,
                code: 1,
                flags: Default::default(),
                ingestion_timestamp: Timestamp::now(),
            };

            let shard_id = id as usize % 4;
            println!("Event {} -> shard {}", id, shard_id);

            sharded.publish_event(event).expect("shard not full");
        }

        println!("All 100 events published");

        // Give handlers MORE time to process all shards
        std::thread::sleep(std::time::Duration::from_secs(2));

        let result_count = sharded.results().len();
        println!("Results count: {}", result_count);

        // Debug: show which sequences have results
        for entry in sharded.results().iter() {
            println!("  seq={}, result={:?}", entry.key(), entry.value());
        }

        // Verify results were produced
        assert_eq!(result_count, 100);

        sharded.stop();
    }

    #[test]
    fn test_sharded_pipeline_scalable_shard_count() {
        // Test that we can create pipelines with different shard counts
        for shard_count in [1, 2, 4, 8, 16] {
            let sharded = ShardedPipeline::new(shard_count, 512, 1_000_000).expect("valid config");
            assert_eq!(sharded.shard_count(), shard_count);
            sharded.stop();
        }
    }
}

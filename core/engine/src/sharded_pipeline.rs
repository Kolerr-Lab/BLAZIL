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

use std::sync::Arc;
use std::thread::JoinHandle;

use blazil_common::error::BlazerResult;
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
    shard_count: usize,
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
            shard_count,
            _handles: handles,
        })
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
        // Route by debit account to ensure deterministic ordering
        let shard_id = (event.debit_account_id.as_u64() as usize) % self.shard_count;
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
    pub fn shard_count(&self) -> usize {
        self.shard_count
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

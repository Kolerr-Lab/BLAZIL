//! Disruptor pipeline: [`PipelineBuilder`], [`Pipeline`], and [`PipelineRunner`].
//!
//! This module wires together the [`RingBuffer`] and the ordered chain of
//! [`EventHandler`]s into a runnable pipeline.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────┐
//! │              Caller thread                           │
//! │  pipeline.publish_event(event) ──► ring_buffer slot  │
//! └──────────────────────────────────────────────────────┘
//!                          │ cursor advances
//!                          ▼
//! ┌──────────────────────────────────────────────────────┐
//! │              Runner thread (busy-spin)               │
//! │  ValidationHandler → RiskHandler → LedgerHandler    │
//! │  → PublishHandler                                    │
//! └──────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use blazil_engine::pipeline::PipelineBuilder;
//! use blazil_engine::handlers::validation::ValidationHandler;
//!
//! let builder = PipelineBuilder::new();
//! let results = builder.results();
//! let (pipeline, runners) = builder
//!     .add_handler(ValidationHandler::new(results))
//!     .build()
//!     .expect("valid capacity");
//!
//! let _handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();
//! // …publish events via pipeline.publish_event(event)…
//! pipeline.stop();
//! ```
//!
//! # Shutdown
//!
//! Call [`Pipeline::stop`] from any thread. The runner finishes its current
//! batch and then exits. Join the returned [`std::thread::JoinHandle`] to
//! wait for all in-flight events to complete.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "macos")]
use libc;

use blazil_common::error::{BlazerError, BlazerResult};
use dashmap::DashMap;
use tracing::instrument;

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;
use crate::result_ring::ResultRing;
use crate::ring_buffer::RingBuffer;
use crate::sequence::Sequence;

// ── PipelineBuilder ───────────────────────────────────────────────────────────

/// Fluent builder for creating a [`Pipeline`] + [`PipelineRunner`] pair.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `capacity` | `65_536` (2¹⁶ slots) |
/// | `handlers` | empty — add at least one before calling `build` |
///
/// # Examples
///
/// ```rust,no_run
/// use blazil_engine::pipeline::PipelineBuilder;
/// use blazil_engine::handlers::validation::ValidationHandler;
///
/// let builder = PipelineBuilder::new().with_capacity(1024);
/// let results = builder.results();
/// let (pipeline, runner) = builder
///     .add_handler(ValidationHandler::new(results))
///     .build()
///     .unwrap();
/// ```
pub struct PipelineBuilder {
    capacity: usize,
    num_workers: usize,
    handlers: Vec<Box<dyn EventHandler>>,
    results: Arc<DashMap<i64, TransactionResult>>,
    /// Override shard_id used for core-affinity pinning when this pipeline is
    /// one shard of many in a `ShardedPipeline`.
    global_shard_id: Option<usize>,
    result_ring: Arc<ResultRing>,
}

impl PipelineBuilder {
    /// Creates a builder with default capacity (`65_536`) and single worker thread.
    pub fn new() -> Self {
        Self {
            capacity: 65_536,
            num_workers: 1,
            handlers: Vec::new(),
            results: Arc::new(DashMap::new()),
            global_shard_id: None,
            result_ring: Arc::new(ResultRing::new(65_536)),
        }
    }

    /// Sets the global shard index used for OS-level core-affinity pinning.
    ///
    /// When building one pipeline per shard inside a `ShardedPipeline`, pass
    /// the logical shard index (0..shard_count) so each worker thread is
    /// pinned to a distinct physical core on Linux and to the correct QoS
    /// class slot on macOS.
    pub fn with_global_shard_id(mut self, shard_id: usize) -> Self {
        self.global_shard_id = Some(shard_id);
        self
    }

    /// Sets the ring buffer capacity (must be a power of two).
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        // Rebuild result_ring if capacity changes and is a power of two.
        // Non-power-of-two values are rejected later in build() by RingBuffer::new.
        if capacity.is_power_of_two() {
            self.result_ring = Arc::new(ResultRing::new(capacity));
        }
        self
    }

    /// Sets the number of worker threads for parallel event processing.
    ///
    /// Each worker processes every Nth event (round-robin sharding).
    /// - `num_workers = 1`: single-threaded (default)
    /// - `num_workers = 8`: 8 workers, each handles every 8th event
    /// - `num_workers = 16`: 16 workers for 16-core bare metal systems
    ///
    /// **Recommendation**: Set to number of physical cores for maximum throughput.
    pub fn with_workers(mut self, num_workers: usize) -> Self {
        assert!(num_workers > 0, "num_workers must be at least 1");
        self.num_workers = num_workers;
        self
    }

    /// Supplies an external results map so handlers and the pipeline share
    /// the same `Arc`.  Call this before adding handlers if you need to pass
    /// the map to handler constructors.
    pub fn with_results(mut self, results: Arc<DashMap<i64, TransactionResult>>) -> Self {
        self.results = results;
        self
    }

    /// Returns a clone of the shared results `Arc` stored in the builder.
    ///
    /// Use this when you need to pass the same map to handler constructors
    /// before calling `build`.
    pub fn results(&self) -> Arc<DashMap<i64, TransactionResult>> {
        Arc::clone(&self.results)
    }

    /// Returns a clone of the shared `ResultRing` `Arc` stored in the builder.
    ///
    /// Pass this to [`handlers::ledger::LedgerHandler::new`] so async TB tasks
    /// write results into the ring instead of the DashMap hot path.
    pub fn result_ring(&self) -> Arc<ResultRing> {
        Arc::clone(&self.result_ring)
    }

    /// Appends an [`EventHandler`] to the pipeline.
    ///
    /// Handlers are called in the order they are added.
    pub fn add_handler(mut self, handler: impl EventHandler + 'static) -> Self {
        self.handlers.push(Box::new(handler));
        self
    }

    /// Builds the pipeline with configured worker threads.
    ///
    /// Returns the `Pipeline` handle and a vector of `PipelineRunner` instances
    /// (one per worker thread). Call `.run()` on each runner to spawn worker threads.
    ///
    /// # Errors
    ///
    /// Returns [`blazil_common::error::BlazerError::ValidationError`] if
    /// `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use blazil_engine::pipeline::PipelineBuilder;
    /// let (pipeline, runners) = PipelineBuilder::new()
    ///     .with_workers(8)
    ///     .build()
    ///     .unwrap();
    ///
    /// // Spawn all worker threads
    /// let handles: Vec<_> = runners.into_iter()
    ///     .map(|runner| runner.run())
    ///     .collect();
    /// ```
    pub fn build(self) -> BlazerResult<(Pipeline, Vec<PipelineRunner>)> {
        let mut ring_buffer = RingBuffer::new(self.capacity)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let results = self.results;

        // Register gating sequences for each worker (one per worker thread)
        let mut worker_gates = Vec::with_capacity(self.num_workers);
        for _ in 0..self.num_workers {
            let gate = ring_buffer.add_gating_sequence();
            worker_gates.push(gate);
        }

        let ring_buffer = Arc::new(ring_buffer);

        let result_ring = self.result_ring;
        let pipeline = Pipeline {
            ring_buffer: Arc::clone(&ring_buffer),
            shutdown: Arc::clone(&shutdown),
            results: Arc::clone(&results),
            result_ring: Arc::clone(&result_ring),
        };

        // Create multiple runners for parallel processing
        let mut runners = Vec::with_capacity(self.num_workers);

        for (worker_idx, gate) in worker_gates.iter().enumerate().take(self.num_workers) {
            // Clone handlers for this worker (Arc internals are shared)
            let handlers = self.handlers.iter().map(|h| h.clone_handler()).collect();

            // Use the globally-assigned shard id when provided (multi-shard setup).
            // Fall back to the per-builder worker index for single-pipeline use.
            let shard_id = self.global_shard_id.unwrap_or(worker_idx);

            runners.push(PipelineRunner {
                ring_buffer: Arc::clone(&ring_buffer),
                handlers,
                shutdown: Arc::clone(&shutdown),
                results: Arc::clone(&results),
                shard_id: worker_idx,
                num_shards: self.num_workers,
                affinity_shard_id: shard_id,
                gating_sequence: Arc::clone(gate),
            });
        }

        Ok((pipeline, runners))
    }
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Pipeline ──────────────────────────────────────────────────────────────────

/// The producer handle to the ring buffer pipeline.
///
/// Obtained from [`PipelineBuilder::build`]. Use [`publish_event`][Pipeline::publish_event]
/// to submit transactions and [`stop`][Pipeline::stop] to initiate a graceful
/// shutdown.
///
/// `Pipeline` is `Clone` — multiple producers may share the same pipeline
/// (but must coordinate externally to maintain the single-writer invariant).
#[derive(Clone)]
pub struct Pipeline {
    ring_buffer: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
    results: Arc<DashMap<i64, TransactionResult>>,
    result_ring: Arc<ResultRing>,
}

impl Pipeline {
    /// Returns a reference to the underlying ring buffer.
    ///
    /// Primarily used in tests to inspect slot state after the runner has
    /// processed events.
    pub fn ring_buffer(&self) -> &Arc<RingBuffer> {
        &self.ring_buffer
    }

    /// Returns a reference to the results map.
    ///
    /// Synchronous rejection results (from `ValidationHandler` / `RiskHandler`)
    /// are stored here. Async TB results live in `result_ring` instead.
    pub fn results(&self) -> &Arc<DashMap<i64, TransactionResult>> {
        &self.results
    }

    /// Returns a reference to the `ResultRing` for async TigerBeetle results.
    ///
    /// The serve thread checks this first (O(1), cache-friendly sequential access)
    /// then falls back to `results()` for the rare synchronous rejection path.
    pub fn result_ring(&self) -> &Arc<ResultRing> {
        &self.result_ring
    }

    /// Signals the runner to exit after finishing its current batch.
    ///
    /// This does **not** block. Join the [`std::thread::JoinHandle`] returned
    /// by [`PipelineRunner::run`] to wait for the runner to finish.
    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Publishes one event to the ring buffer.
    ///
    /// Writes `event` to the next available slot and advances the cursor so
    /// the runner can process it. Returns the sequence number of the published
    /// event.
    ///
    /// # Errors
    ///
    /// Currently infallible (returns `Ok(seq)`). Reserved for future
    /// back-pressure implementations.
    ///
    /// # Safety
    ///
    /// Must be called from a **single producer** thread. Concurrent calls
    /// from multiple threads violate the single-writer invariant and would
    /// cause data races.
    #[instrument(skip(self, event), fields(transaction_id = %event.transaction_id))]
    pub fn publish_event(&self, event: TransactionEvent) -> BlazerResult<i64> {
        // Check if ring buffer has capacity before claiming a slot.
        if !self.ring_buffer.has_available_capacity() {
            return Err(BlazerError::RingBufferFull { retry_after_ms: 1 });
        }

        let seq = self.ring_buffer.next_sequence();

        // SAFETY: single producer — we just claimed `seq` via `next_sequence()`.
        // No other thread may write to this slot until we call `publish`.
        unsafe {
            *self.ring_buffer.get_mut(seq) = event;
        }

        // Release fence: the slot write above must be visible to the runner
        // before the cursor advances. `publish` issues a Release store.
        self.ring_buffer.publish(seq);

        Ok(seq)
    }
}

// ── PipelineRunner ────────────────────────────────────────────────────────────

/// The consumer that drives handlers around the ring buffer.
///
/// Obtained from [`PipelineBuilder::build`]. Call [`run`][PipelineRunner::run]
/// to spawn the runner on a dedicated OS thread.
///
/// # Busy-spin
///
/// The runner never sleeps. It calls [`std::hint::spin_loop`] when no new
/// events are available, yielding the CPU pipeline hint without a context
/// switch. This minimises latency at the cost of a dedicated CPU core.
pub struct PipelineRunner {
    ring_buffer: Arc<RingBuffer>,
    handlers: Vec<Box<dyn EventHandler>>,
    shutdown: Arc<AtomicBool>,
    #[allow(dead_code)]
    results: Arc<DashMap<i64, TransactionResult>>,
    /// Shard ID for this worker (0..num_shards) — used for ring-buffer position tracking.
    shard_id: usize,
    /// Total number of worker shards
    num_shards: usize,
    /// Global shard index used exclusively for OS-level core-affinity pinning.
    /// Equals `shard_id` for single-pipeline builds; set to the outer shard index
    /// when this runner is one of many in a `ShardedPipeline`.
    affinity_shard_id: usize,
    /// This worker's dedicated gating sequence
    gating_sequence: Arc<Sequence>,
}

impl PipelineRunner {
    /// Spawns the runner on a new OS thread.
    ///
    /// Returns a [`std::thread::JoinHandle`] you can `join` after calling
    /// [`Pipeline::stop`] to wait for all in-flight events to complete.
    ///
    /// # Panics
    ///
    /// Panics if the OS fails to spawn the thread (OS resource exhaustion).
    pub fn run(mut self) -> std::thread::JoinHandle<()> {
        let shard_id = self.shard_id;
        let num_shards = self.num_shards;
        let affinity_shard_id = self.affinity_shard_id;

        std::thread::Builder::new()
            .name(format!("blazil-shard-{}", affinity_shard_id))
            .spawn(move || {
                // ── OS-aware thread priority & core affinity ─────────────────────
                // macOS: promote to User-Interactive QoS → scheduler prefers
                // P-cores and grants highest scheduling priority.
                #[cfg(target_os = "macos")]
                unsafe {
                    // QOS_CLASS_USER_INTERACTIVE = 0x21 (not re-exported by libc).
                    extern "C" {
                        fn pthread_set_qos_class_self_np(
                            qos_class: libc::c_uint,
                            relative_priority: libc::c_int,
                        ) -> libc::c_int;
                    }
                    pthread_set_qos_class_self_np(0x21, 0);
                }

                // Linux (DO): hard-pin each shard to a dedicated core.
                // Core 0 is reserved for network IRQs; shards start at core 1.
                // Uses affinity_shard_id (the global shard index) so all shards
                // in a ShardedPipeline land on distinct physical cores.
                #[cfg(target_os = "linux")]
                {
                    if let Some(core_ids) = core_affinity::get_core_ids() {
                        if let Some(id) = core_ids.get((affinity_shard_id % core_ids.len()) + 1) {
                            core_affinity::set_for_current(*id);
                        }
                    }
                }

                for handler in &mut self.handlers {
                    handler.on_start();
                }

                // Start at first sequence owned by this shard
                // Shard 0 starts at 0, shard 1 at 1, etc.
                let mut consumer_seq = (shard_id as i64) - 1;

                loop {
                    // Acquire load: pairs with the Release store in `RingBuffer::publish`.
                    let cursor = self.ring_buffer.cursor().get();

                    if cursor > consumer_seq {
                        // Process events belonging to this shard (every Nth event)
                        while consumer_seq < cursor {
                            consumer_seq += num_shards as i64;

                            // Skip if we've advanced beyond current cursor
                            if consumer_seq > cursor {
                                consumer_seq -= num_shards as i64;
                                break;
                            }

                            let end_of_batch = consumer_seq >= cursor - (num_shards as i64 - 1);

                            // SAFETY: Each worker processes disjoint sequences (shard_id + k*num_shards).
                            // The producer publishes with Release semantics. No data races.
                            let event = unsafe { &mut *self.ring_buffer.get_mut(consumer_seq) };

                            for handler in &mut self.handlers {
                                handler.on_event(event, consumer_seq, end_of_batch);
                            }
                        }

                        // Update this worker's gating sequence (lock-free, no contention)
                        // Producer computes MIN across all workers' gating sequences
                        self.gating_sequence.set(consumer_seq);
                    } else if self.shutdown.load(Ordering::Acquire) {
                        // Check shutdown only when idle to avoid splitting a batch.
                        break;
                    } else {
                        std::hint::spin_loop();
                    }
                }

                for handler in &mut self.handlers {
                    handler.on_shutdown();
                }
            })
            .expect("failed to spawn pipeline worker thread")
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::mock::InMemoryLedgerClient;

    use super::*;
    use crate::event::TransactionResult;
    use crate::handlers::ledger::LedgerHandler;
    use crate::handlers::publish::PublishHandler;
    use crate::handlers::risk::RiskHandler;
    use crate::handlers::validation::ValidationHandler;

    // ── helpers ────────────────────────────────────────────────────────────────

    /// Creates a mock ledger client pre-seeded with one debit account and one
    /// credit account. Returns (client, debit_id, credit_id, runtime).
    fn build_client() -> (
        Arc<InMemoryLedgerClient>,
        AccountId,
        AccountId,
        Arc<tokio::runtime::Runtime>,
    ) {
        let rt = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
        let client = Arc::new(InMemoryLedgerClient::new());

        use blazil_common::currency::parse_currency;

        let usd = parse_currency("USD").expect("USD");

        let debit_id = rt.block_on(async {
            let acc = Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            );
            client
                .create_account(acc)
                .await
                .expect("create debit account")
        });
        let credit_id = rt.block_on(async {
            let usd2 = parse_currency("USD").expect("USD");
            let acc = Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd2,
                1,
                AccountFlags::default(),
            );
            client
                .create_account(acc)
                .await
                .expect("create credit account")
        });

        (client, debit_id, credit_id, rt)
    }

    fn make_event(debit_id: AccountId, credit_id: AccountId) -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            debit_id,
            credit_id,
            10_000_u64, // $100.00 in cents
            LedgerId::USD,
            1,
        )
    }

    fn build_full_pipeline(
        client: Arc<InMemoryLedgerClient>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> (Pipeline, Vec<std::thread::JoinHandle<()>>) {
        // $1,000,000.00 in cents.
        let max_amount_units: u64 = 100_000_000;

        let builder = PipelineBuilder::new().with_capacity(1024);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(max_amount_units, Arc::clone(&results)))
            .add_handler(LedgerHandler::new(client, runtime, Arc::clone(&results)))
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("valid pipeline");

        let handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();
        (pipeline, handles)
    }

    // ── integration tests ──────────────────────────────────────────────────────

    /// Wait for the event slot to have a result, polling up to a deadline.
    fn wait_for_result(
        results: &Arc<DashMap<i64, TransactionResult>>,
        seq: i64,
        deadline: Duration,
    ) -> Option<TransactionResult> {
        let start = std::time::Instant::now();
        loop {
            if let Some(r) = results.get(&seq) {
                return Some(r.value().clone());
            }
            if start.elapsed() >= deadline {
                return None;
            }
            std::hint::spin_loop();
        }
    }

    #[test]
    fn valid_transaction_is_committed() {
        let (client, debit_id, credit_id, runtime) = build_client();
        let (pipeline, handles) = build_full_pipeline(client, runtime);

        let event = make_event(debit_id, credit_id);
        let seq = pipeline.publish_event(event).expect("publish");

        let result = wait_for_result(pipeline.results(), seq, Duration::from_secs(5));

        pipeline.stop();
        for h in handles {
            h.join().expect("runner panicked");
        }

        assert!(
            matches!(result, Some(TransactionResult::Committed { .. })),
            "expected Committed, got {:?}",
            result
        );
    }

    #[test]
    fn transaction_with_nil_ids_is_rejected_by_validation() {
        // Use a pipeline with NO LedgerHandler so we don't need real accounts.
        let builder = PipelineBuilder::new().with_capacity(1024);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");
        let handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

        // zero TransactionId — ValidationHandler rejects
        let mut event = TransactionEvent::new(
            TransactionId::from_u64(0), // zero = nil sentinel
            AccountId::new(),
            AccountId::new(),
            50_00_u64, // $50.00
            LedgerId::USD,
            1,
        );
        event.sequence = -1;

        let seq = pipeline.publish_event(event).expect("publish");
        let result = wait_for_result(pipeline.results(), seq, Duration::from_secs(5));

        pipeline.stop();
        for h in handles {
            h.join().expect("runner panicked");
        }

        assert!(
            matches!(result, Some(TransactionResult::Rejected { .. })),
            "expected Rejected, got {:?}",
            result
        );
    }

    #[test]
    fn transaction_over_risk_limit_is_rejected() {
        let builder = PipelineBuilder::new().with_capacity(1024);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(1_00_u64, Arc::clone(&results))) // max = $1.00
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");
        let handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

        // Amount ($500) >> risk limit ($1)
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            50_000_u64, // $500.00
            LedgerId::USD,
            1,
        );
        // Flag as requiring risk check
        event.flags.set_requires_risk_check(true);

        let seq = pipeline.publish_event(event).expect("publish");
        let result = wait_for_result(pipeline.results(), seq, Duration::from_secs(5));

        pipeline.stop();
        for h in handles {
            h.join().expect("runner panicked");
        }

        assert!(
            matches!(result, Some(TransactionResult::Rejected { .. })),
            "expected Rejected, got {:?}",
            result
        );
    }

    #[test]
    fn multiple_valid_transactions_are_all_committed() {
        let (client, debit_id, credit_id, runtime) = build_client();
        let (pipeline, handles) = build_full_pipeline(client, runtime);

        const N: usize = 8;
        let mut seqs = Vec::with_capacity(N);
        for _ in 0..N {
            let event = make_event(debit_id, credit_id);
            let seq = pipeline.publish_event(event).expect("publish");
            seqs.push(seq);
        }

        let results_map = Arc::clone(pipeline.results());
        let results: Vec<_> = seqs
            .iter()
            .map(|&s| wait_for_result(&results_map, s, Duration::from_secs(10)))
            .collect();

        pipeline.stop();
        for h in handles {
            h.join().expect("runner panicked");
        }

        for (i, result) in results.into_iter().enumerate() {
            assert!(
                matches!(result, Some(TransactionResult::Committed { .. })),
                "event {i}: expected Committed, got {:?}",
                result
            );
        }
    }

    // ── builder unit tests ────────────────────────────────────────────────────

    #[test]
    fn builder_default_capacity_is_65536() {
        let builder = PipelineBuilder::new();
        assert_eq!(builder.capacity, 65_536);
    }

    #[test]
    fn builder_with_capacity_overrides_default() {
        let builder = PipelineBuilder::new().with_capacity(1024);
        assert_eq!(builder.capacity, 1024);
    }

    #[test]
    fn builder_non_power_of_two_capacity_fails() {
        let builder = PipelineBuilder::new().with_capacity(1000);
        let results = builder.results();
        let result = builder.add_handler(ValidationHandler::new(results)).build();
        assert!(result.is_err());
    }
}

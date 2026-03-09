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
//! let (pipeline, runner) = PipelineBuilder::new()
//!     .add_handler(ValidationHandler)
//!     .build()
//!     .expect("valid capacity");
//!
//! let _handle = runner.run();
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

use blazil_common::error::BlazerResult;
use tracing::instrument;

use crate::event::TransactionEvent;
use crate::handler::EventHandler;
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
/// let (pipeline, runner) = PipelineBuilder::new()
///     .with_capacity(1024)
///     .add_handler(ValidationHandler)
///     .build()
///     .unwrap();
/// ```
pub struct PipelineBuilder {
    capacity: usize,
    handlers: Vec<Box<dyn EventHandler>>,
}

impl PipelineBuilder {
    /// Creates a builder with default capacity (`65_536`).
    pub fn new() -> Self {
        Self {
            capacity: 65_536,
            handlers: Vec::new(),
        }
    }

    /// Sets the ring buffer capacity (must be a power of two).
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Appends an [`EventHandler`] to the pipeline.
    ///
    /// Handlers are called in the order they are added.
    pub fn add_handler(mut self, handler: impl EventHandler + 'static) -> Self {
        self.handlers.push(Box::new(handler));
        self
    }

    /// Builds the pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`blazil_common::error::BlazerError::ValidationError`] if
    /// `capacity` is zero or not a power of two.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use blazil_engine::pipeline::PipelineBuilder;
    /// use blazil_engine::handlers::validation::ValidationHandler;
    ///
    /// PipelineBuilder::new().add_handler(ValidationHandler).build().unwrap();
    /// ```
    pub fn build(self) -> BlazerResult<(Pipeline, PipelineRunner)> {
        let ring_buffer = Arc::new(RingBuffer::new(self.capacity)?);
        let shutdown = Arc::new(AtomicBool::new(false));

        let pipeline = Pipeline {
            ring_buffer: Arc::clone(&ring_buffer),
            shutdown: Arc::clone(&shutdown),
        };
        let runner = PipelineRunner {
            ring_buffer,
            handlers: self.handlers,
            shutdown,
        };

        Ok((pipeline, runner))
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
pub struct Pipeline {
    ring_buffer: Arc<RingBuffer>,
    shutdown: Arc<AtomicBool>,
}

impl Pipeline {
    /// Returns a reference to the underlying ring buffer.
    ///
    /// Primarily used in tests to inspect slot state after the runner has
    /// processed events.
    pub fn ring_buffer(&self) -> &Arc<RingBuffer> {
        &self.ring_buffer
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
        std::thread::spawn(move || {
            for handler in &mut self.handlers {
                handler.on_start();
            }

            let mut consumer_seq = Sequence::INITIAL_VALUE; // −1

            loop {
                // Acquire load: pairs with the Release store in `RingBuffer::publish`.
                let cursor = self.ring_buffer.cursor().get();

                if cursor > consumer_seq {
                    // Drain all published-but-unprocessed events.
                    while consumer_seq < cursor {
                        consumer_seq += 1;
                        let end_of_batch = consumer_seq == cursor;

                        // SAFETY: only the runner reads slots ≤ cursor after
                        // the cursor was published (Release). The producer will
                        // not reuse this slot until the ring wraps around (which
                        // requires 2^N more events than the current window).
                        let event = unsafe { &mut *self.ring_buffer.get_mut(consumer_seq) };

                        for handler in &mut self.handlers {
                            handler.on_event(event, consumer_seq, end_of_batch);
                        }
                    }
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
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::mock::InMemoryLedgerClient;
    use rust_decimal::Decimal;

    use super::*;
    use crate::event::{EventFlags, TransactionResult};
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
        let usd = parse_currency("USD").expect("USD");
        let amount = Amount::new(Decimal::new(100_00, 2), usd).expect("amount");
        TransactionEvent::new(
            TransactionId::new(),
            debit_id,
            credit_id,
            amount,
            LedgerId::USD,
            1,
        )
    }

    fn build_full_pipeline(
        client: Arc<InMemoryLedgerClient>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> (Pipeline, std::thread::JoinHandle<()>) {
        let max_amount = Amount::new(
            Decimal::new(1_000_000_00, 2),
            parse_currency("USD").expect("USD"),
        )
        .expect("max amount");

        let (pipeline, runner) = PipelineBuilder::new()
            .with_capacity(1024)
            .add_handler(ValidationHandler)
            .add_handler(RiskHandler::new(max_amount))
            .add_handler(LedgerHandler::new(client, runtime))
            .add_handler(PublishHandler::new())
            .build()
            .expect("valid pipeline");

        let handle = runner.run();
        (pipeline, handle)
    }

    // ── integration tests ──────────────────────────────────────────────────────

    /// Wait for the event slot to have a result, polling up to a deadline.
    fn wait_for_result(
        ring_buffer: &Arc<RingBuffer>,
        seq: i64,
        deadline: Duration,
    ) -> Option<TransactionResult> {
        let start = std::time::Instant::now();
        loop {
            // SAFETY: we only read, and the runner is done writing when result is Some.
            let result = unsafe { &*ring_buffer.get(seq) }.result.clone();
            if result.is_some() {
                return result;
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
        let (pipeline, handle) = build_full_pipeline(client, runtime);

        let event = make_event(debit_id, credit_id);
        let seq = pipeline.publish_event(event).expect("publish");

        let result = wait_for_result(pipeline.ring_buffer(), seq, Duration::from_secs(5));

        pipeline.stop();
        handle.join().expect("runner panicked");

        assert!(
            matches!(result, Some(TransactionResult::Committed { .. })),
            "expected Committed, got {:?}",
            result
        );
    }

    #[test]
    fn transaction_with_nil_ids_is_rejected_by_validation() {
        // Use a pipeline with NO LedgerHandler so we don't need real accounts.
        let (pipeline, runner) = PipelineBuilder::new()
            .with_capacity(1024)
            .add_handler(ValidationHandler)
            .add_handler(PublishHandler::new())
            .build()
            .expect("pipeline");
        let handle = runner.run();

        // nil TransactionId — ValidationHandler rejects
        let usd = parse_currency("USD").expect("USD");
        let amount = Amount::new(Decimal::new(50_00, 2), usd).expect("amount");
        let mut event = TransactionEvent::new(
            TransactionId::from_bytes([0u8; 16]), // nil UUID
            AccountId::new(),
            AccountId::new(),
            amount,
            LedgerId::USD,
            1,
        );
        event.sequence = -1;

        let seq = pipeline.publish_event(event).expect("publish");
        let result = wait_for_result(pipeline.ring_buffer(), seq, Duration::from_secs(5));

        pipeline.stop();
        handle.join().expect("runner panicked");

        assert!(
            matches!(result, Some(TransactionResult::Rejected { .. })),
            "expected Rejected, got {:?}",
            result
        );
    }

    #[test]
    fn transaction_over_risk_limit_is_rejected() {
        let tiny_max =
            Amount::new(Decimal::new(1_00, 2), parse_currency("USD").expect("USD")).expect("max");

        let (pipeline, runner) = PipelineBuilder::new()
            .with_capacity(1024)
            .add_handler(ValidationHandler)
            .add_handler(RiskHandler::new(tiny_max))
            .add_handler(PublishHandler::new())
            .build()
            .expect("pipeline");
        let handle = runner.run();

        let usd = parse_currency("USD").expect("USD");
        // Amount ($500) >> risk limit ($1)
        let amount = Amount::new(Decimal::new(500_00, 2), usd).expect("amount");
        let mut event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::new(),
            AccountId::new(),
            amount,
            LedgerId::USD,
            1,
        );
        // Flag as requiring risk check
        event.flags = EventFlags {
            requires_risk_check: true,
            ..EventFlags::default()
        };

        let seq = pipeline.publish_event(event).expect("publish");
        let result = wait_for_result(pipeline.ring_buffer(), seq, Duration::from_secs(5));

        pipeline.stop();
        handle.join().expect("runner panicked");

        assert!(
            matches!(result, Some(TransactionResult::Rejected { .. })),
            "expected Rejected, got {:?}",
            result
        );
    }

    #[test]
    fn multiple_valid_transactions_are_all_committed() {
        let (client, debit_id, credit_id, runtime) = build_client();
        let (pipeline, handle) = build_full_pipeline(client, runtime);

        const N: usize = 8;
        let mut seqs = Vec::with_capacity(N);
        for _ in 0..N {
            let event = make_event(debit_id, credit_id);
            let seq = pipeline.publish_event(event).expect("publish");
            seqs.push(seq);
        }

        let rb = Arc::clone(pipeline.ring_buffer());
        let results: Vec<_> = seqs
            .iter()
            .map(|&s| wait_for_result(&rb, s, Duration::from_secs(10)))
            .collect();

        pipeline.stop();
        handle.join().expect("runner panicked");

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
        let result = PipelineBuilder::new()
            .with_capacity(1000)
            .add_handler(ValidationHandler)
            .build();
        assert!(result.is_err());
    }
}

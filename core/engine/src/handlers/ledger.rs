//! Ledger commit handler with batching optimization.
//!
//! [`LedgerHandler`] is the **third** stage in the pipeline. It is where
//! money actually moves: it builds a [`blazil_ledger::transfer::Transfer`]
//! from the event and calls [`LedgerClient::create_transfers_batch`] to commit
//! batches to TigerBeetle.
//!
//! # Batching for 100× throughput
//!
//! TigerBeetle VSR consensus cost is per-batch, not per-transfer. A single
//! transfer costs ~1.6ms in VSR overhead; 100 transfers in one batch also cost
//! ~1.6ms. [`LedgerHandler`] accumulates transfers until `end_of_batch` is true
//! or batch size reaches [`MAX_BATCH`], then flushes via
//! [`LedgerClient::create_transfers_batch`].
//!
//! Deferred events (those in the batch before the flush trigger) have their
//! `result` field set to `None` during `on_event`. After the batch write
//! completes, results are written back to those ring buffer slots via raw
//! pointers. This is safe because:
//!
//! 1. Pointers reference **previous** `on_event` call slots (already processed).
//! 2. The producer cannot reclaim those slots until `gating_sequence` advances.
//! 3. `gating_sequence` only advances **after** all handlers (including this one)
//!    return from the full batch loop.
//! 4. All accesses are on the single dedicated runner thread (no data races).
//!
//! # Async-in-sync
//!
//! The handler trait requires synchronous `on_event` calls (the pipeline
//! thread must not park itself on an async executor). `LedgerHandler` uses
//! `tokio::runtime::Runtime::block_on` to drive the async `LedgerClient`
//! call to completion on the calling thread. This is correct because the
//! handler thread is a **dedicated pinned thread** — blocking it for an I/O
//! round-trip is intentional. The async runtime handles the actual I/O
//! without holding any system threads for the full duration.
//!
//! # Latency diagnostics
//!
//! Every batch flush logs `tb_elapsed_ms` and `batch_size`.  On a healthy 3-node
//! DO VSR cluster, batch latency should be 1–3 ms regardless of batch size (up
//! to 8,190 transfers).  Values > 5 ms emit a `WARN`.

use std::mem;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blazil_common::ids::TransferId;
use blazil_common::timestamp::Timestamp;
use blazil_ledger::client::LedgerClient;
use blazil_ledger::convert::{ledger_id_to_currency, minor_units_to_amount};
use blazil_ledger::transfer::Transfer;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use crate::event::{TransactionEvent, TransactionResult};
use crate::handler::EventHandler;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum transfers per TigerBeetle batch (TB hard limit is 8,190).
/// Flushing at this size amortises VSR consensus cost across the maximum
/// number of transfers per round-trip, yielding peak throughput.
const MAX_TB_BATCH_SIZE: usize = 8_190;

/// Flush timeout in microseconds. Batches that haven't reached
/// `MAX_TB_BATCH_SIZE` are flushed after this interval regardless.
/// 500 µs ≈ the DO-region RTT between nodes, so the timeout adds
/// zero extra latency relative to the network round-trip.
const BATCH_FLUSH_TIMEOUT_US: u64 = 500;

// ── LedgerHandler ─────────────────────────────────────────────────────────────

/// Commits transactions to TigerBeetle in batches.
///
/// Wraps any [`LedgerClient`] implementation. In tests, use
/// [`blazil_ledger::mock::InMemoryLedgerClient`]; in production, use
/// `TigerBeetleClient` (feature-gated in `blazil-ledger`).
pub struct LedgerHandler<C: LedgerClient> {
    client: Arc<C>,
    runtime: Arc<tokio::runtime::Runtime>,
    results: Arc<DashMap<i64, TransactionResult>>,
    /// Timestamp when the first transfer was added to the current batch.
    /// Used for time-based flush trigger.
    batch_started_at: Option<Instant>,
    /// Accumulated transfers waiting to be flushed.
    deferred_transfers: Vec<Transfer>,
    /// Metadata for deferred events: sequence numbers only.
    /// Results are written to the external results map after batch flush.
    deferred_sequences: Vec<i64>,
    /// Count of currently in-flight TB `runtime.spawn()` tasks.
    /// Incremented just before spawn, decremented inside the async task when
    /// it completes. Exposed via `active_tasks()` for external monitoring.
    /// A persistently growing value means TB is building backpressure on disk.
    active_tasks: Arc<AtomicUsize>,
}

impl<C: LedgerClient> Clone for LedgerHandler<C> {
    fn clone(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            runtime: Arc::clone(&self.runtime),
            results: Arc::clone(&self.results),
            // Reset worker-local state for new worker thread
            batch_started_at: None,
            deferred_transfers: Vec::new(),
            deferred_sequences: Vec::new(),
            // Share the same counter — all shards feed into one number.
            active_tasks: Arc::clone(&self.active_tasks),
        }
    }
}

impl<C: LedgerClient> LedgerHandler<C> {
    /// Creates a new `LedgerHandler`.
    ///
    /// - `client` — the [`LedgerClient`] that writes transfers to TigerBeetle.
    /// - `runtime` — a Tokio `Runtime` used to drive async calls synchronously
    ///   from the handler thread.
    /// - `results` — the shared results map where transaction outcomes are stored.
    pub fn new(
        client: Arc<C>,
        runtime: Arc<tokio::runtime::Runtime>,
        results: Arc<DashMap<i64, TransactionResult>>,
    ) -> Self {
        Self {
            client,
            batch_started_at: None,
            runtime,
            results,
            deferred_transfers: Vec::with_capacity(MAX_TB_BATCH_SIZE),
            deferred_sequences: Vec::with_capacity(MAX_TB_BATCH_SIZE),
            active_tasks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns a reference to the shared active-task counter.
    ///
    /// Clone the `Arc` before calling `PipelineBuilder::add_handler` so that
    /// an external monitor (e.g. the bench client drain loop) can observe how
    /// many TigerBeetle batches are currently in-flight.
    ///
    /// ```ignore
    /// let handler = LedgerHandler::new(client, rt, results);
    /// let active = Arc::clone(handler.active_tasks());
    /// builder = builder.add_handler(handler);
    /// // ... later, in a heartbeat:
    /// println!("active_tb_tasks={}", active.load(std::sync::atomic::Ordering::Relaxed));
    /// ```
    pub fn active_tasks(&self) -> &Arc<AtomicUsize> {
        &self.active_tasks
    }
}

impl<C: LedgerClient + Send + Sync + 'static> EventHandler for LedgerHandler<C> {
    fn on_event(&mut self, event: &mut TransactionEvent, sequence: i64, end_of_batch: bool) {
        // Rule 1: skip if already rejected.
        if self.results.contains_key(&sequence) {
            return;
        }

        // Reconstruct Amount from minor units + ledger currency at this boundary.
        let amount = match ledger_id_to_currency(&event.ledger_id)
            .and_then(|c| minor_units_to_amount(event.amount_units as u128, c))
        {
            Ok(a) => a,
            Err(e) => {
                error!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    error = %e,
                    "LedgerHandler: failed to reconstruct Amount from amount_units"
                );
                self.results
                    .insert(sequence, TransactionResult::Rejected { reason: e });
                return;
            }
        };

        // Build a Transfer from the event fields.
        let transfer = match Transfer::new(
            TransferId::new(),
            event.debit_account_id,
            event.credit_account_id,
            amount,
            event.ledger_id,
            event.code,
        ) {
            Ok(t) => t,
            Err(e) => {
                error!(
                    sequence,
                    transaction_id = %event.transaction_id,
                    error = %e,
                    "LedgerHandler: failed to construct Transfer"
                );
                self.results
                    .insert(sequence, TransactionResult::Rejected { reason: e });
                return;
            }
        };

        // Decide: defer or flush?
        // Check if batch age exceeds timeout (handles moderate load where batch never fills)
        let batch_age_exceeded = self
            .batch_started_at
            .map(|started| started.elapsed() >= Duration::from_micros(BATCH_FLUSH_TIMEOUT_US))
            .unwrap_or(false);

        // NOTE: flush when adding *this* transfer would reach the limit (+1 for current).
        // Without the -1 the batch would include MAX_TB_BATCH_SIZE deferred + 1 current =
        // MAX_TB_BATCH_SIZE+1 which exceeds TigerBeetle’s hard cap of 8,190 per request.
        let should_flush = end_of_batch
            || self.deferred_transfers.len() + 1 >= MAX_TB_BATCH_SIZE
            || batch_age_exceeded;

        if !should_flush {
            // Defer: queue the transfer and sequence number for batch flush.
            // Start batch timer on first deferred transfer
            if self.deferred_transfers.is_empty() {
                self.batch_started_at = Some(Instant::now());
            }

            self.deferred_transfers.push(transfer);
            self.deferred_sequences.push(sequence);
            debug!(
                sequence,
                transaction_id = %event.transaction_id,
                deferred_count = self.deferred_transfers.len(),
                "LedgerHandler: deferring transfer (waiting for batch flush)"
            );
            return;
        }

        // Flush: drain deferred queue, append current transfer, make one TB call.
        let prev_n = self.deferred_transfers.len();
        let prev_sequences = mem::take(&mut self.deferred_sequences);
        let mut all_transfers = mem::take(&mut self.deferred_transfers);
        // Pre-allocate replacement buffers immediately so the next batch never
        // reallocates on the hot path.  The batch buffer is always fixed-size.
        self.deferred_transfers = Vec::with_capacity(MAX_TB_BATCH_SIZE);
        self.deferred_sequences = Vec::with_capacity(MAX_TB_BATCH_SIZE);
        all_transfers.push(transfer);

        let batch_size = all_transfers.len();
        let batch_age_ms = self
            .batch_started_at
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);
        let client = Arc::clone(&self.client);

        // Reset batch timer
        self.batch_started_at = None;

        debug!(
            sequence,
            batch_size, batch_age_ms, end_of_batch, "LedgerHandler: spawning async TB batch"
        );

        // ── Non-blocking TB dispatch ────────────────────────────────────────────
        //
        // CRITICAL CHANGE: we spawn an async task instead of blocking via
        // block_on(). The runner thread returns IMMEDIATELY from on_event,
        // advances the ring-buffer gating sequence, and begins accumulating
        // the NEXT batch while TigerBeetle is processing the current one.
        //
        // This pipelines consecutive TB batches: batch N+1 is in the TB TCP
        // send buffer before batch N VSR consensus completes. Effective TPS
        // is then (N_concurrent_batches × batch_size) / TB_RTT instead of
        // batch_size / TB_RTT, keeping throughput flat even as TB_RTT grows.
        //
        // The serve thread’s VecDeque front-check is still correct because the
        // Zig TB client processes requests for a single connection in submission
        // order — batch 0 results always arrive before batch 1 results.
        let results_map = Arc::clone(&self.results);
        let current_seq = sequence;
        // Track in-flight count. Increment BEFORE spawn so the monitoring
        // counter is never transiently zero between dispatched batches.
        let active_tasks = Arc::clone(&self.active_tasks);
        active_tasks.fetch_add(1, Ordering::Relaxed);
        self.runtime.spawn(async move {
            let tb_t0 = Instant::now();
            let tb_results = client.create_transfers_batch(all_transfers).await;
            let tb_elapsed_ms = tb_t0.elapsed().as_millis();
            // Decrement after TB response arrives.
            active_tasks.fetch_sub(1, Ordering::Relaxed);

            if tb_elapsed_ms > 5 {
                warn!(
                    batch_size,
                    tb_elapsed_ms, "LedgerHandler: SLOW batch write (>5 ms)"
                );
            } else {
                info!(batch_size, tb_elapsed_ms, "LedgerHandler: batch committed");
            }

            // Write deferred sequences’ results to the shared map.
            for (i, seq) in prev_sequences.iter().enumerate() {
                let tr = match &tb_results[i] {
                    Ok(transfer_id) => {
                        debug!(sequence = seq, %transfer_id, "LedgerHandler: deferred committed");
                        TransactionResult::Committed {
                            transfer_id: *transfer_id,
                            timestamp: Timestamp::now(),
                        }
                    }
                    Err(e) => {
                        debug!(sequence = seq, error = %e, "LedgerHandler: deferred rejected");
                        TransactionResult::Rejected { reason: e.clone() }
                    }
                };
                results_map.insert(*seq, tr);
            }

            // Write current event’s result.
            let tr = match &tb_results[prev_n] {
                Ok(transfer_id) => {
                    debug!(sequence = current_seq, %transfer_id, "LedgerHandler: current committed");
                    TransactionResult::Committed {
                        transfer_id: *transfer_id,
                        timestamp: Timestamp::now(),
                    }
                }
                Err(e) => {
                    debug!(sequence = current_seq, error = %e, "LedgerHandler: current rejected");
                    TransactionResult::Rejected { reason: e.clone() }
                }
            };
            results_map.insert(current_seq, tr);
        });
    }

    fn clone_handler(&self) -> Box<dyn EventHandler> {
        Box::new(self.clone())
    }
}

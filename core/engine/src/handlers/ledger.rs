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
use crate::result_ring::ResultRing;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum transfers per TigerBeetle batch (TB hard limit is 8,190).
/// Flushing at this size amortises VSR consensus cost across the maximum
/// number of transfers per round-trip, yielding peak throughput.
const MAX_TB_BATCH_SIZE: usize = 8_190;

/// Flush timeout in microseconds. Batches that haven't reached
/// `MAX_TB_BATCH_SIZE` are flushed after this interval regardless.
/// 500 µs ≈ the DO intra-region RTT so partial batches are held just
/// long enough to accumulate more transfers without wasting a VSR round.
const BATCH_FLUSH_TIMEOUT_US: u64 = 3_000;

/// Maximum retry attempts for a transient TB transport error (view-change,
/// session reset).  Each attempt is separated by an exponential backoff:
///   attempt 1 →  50 ms
///   attempt 2 → 100 ms
///   attempt 3 → 200 ms
///   attempt 4 → 400 ms
///   attempt 5 → 800 ms (final)
///
/// Total worst-case wait: ~1 550 ms, well within the VSR view-change window
/// (~1-3 s on well-connected nodes).  After this the batch is marked rejected
/// so the pipeline does not stall forever.
const MAX_TRANSIENT_RETRIES: u32 = 5;

/// Base delay in milliseconds for the first retry backoff.
/// Subsequent delays double: 50 → 100 → 200 → 400 → 800 ms.
const TRANSIENT_BACKOFF_BASE_MS: u64 = 50;

/// Maximum number of concurrent in-flight TigerBeetle async tasks.
///
/// This is the key backpressure constant that prevents unbounded growth.
///
/// # Why this prevents TPS decay
///
/// Without a cap, as TB VSR RTT grows (journal growth, DO disk I/O pressure,
/// compaction), the runner thread spawns new batches faster than TB can
/// complete old ones. Each in-flight task holds:
///   - `prev_sequences`: Vec<i64> up to 8,189 entries (65 KB)
///   - `all_transfers`: Vec<Transfer> up to 8,189 entries (~800 KB)
///   - A clone of `results_map` Arc (DashMap growing with 8,190 new entries)
///
/// With no cap: 100 concurrent tasks × ~865 KB/task = ~85 MB heap pressure
/// + DashMap with 819,000 entries → severe cache thrashing → each drain
///   call takes longer → serve thread stalls longer → less time polling Aeron
///   → fewer new events/sec → TPS decay. Positive feedback loop.
///
/// With cap=32: max 32 × 8,190 = 262,080 DashMap entries and ~27 MB heap for
/// in-flight task data. Still bounded; the DO NVMe I/O bandwidth (~500 MB/s)
/// is the outer limit, not the DashMap. Increased from 16 → 32 to fill the
/// pipeline deeper when TB VSR RTT is ~1 ms (8,190 transfers × 1000 µs/ms
/// / 1,000,000 µs = 8,190,000 / 1M ≈ 8.2 TPS per task → 32 tasks ≈ 262 K
/// in-flight keeps 8 shards × 32 K window fed continuously).
///
/// With cap=16: max 16 × 8,190 = 131,040 DashMap entries and ~13.6 MB heap for
/// in-flight task data. Bounded at all TB RTT values. TPS stays flat.
///
/// Maximum number of concurrent in-flight TigerBeetle async tasks **per shard**.
///
/// With the 4-client pool (2 shards per client):
///   per-client in-flight = 2 shards × 8 = 16 batches
///   total in-flight       = 4 clients × 16 = 64 batches
///   max simultaneous transfers = 64 × 8,190 = 524,160
///
/// Keeping 16 per shard (old value) would give 32 per client — too many
/// tasks competing for one io_uring queue, causing scheduling overhead.
const MAX_CONCURRENT_BATCHES: usize = 8;

/// Spin iterations before yielding in the concurrent-batch backpressure wait.
/// ~512 spins ≈ ~1 µs at 3 GHz — minimises OS context switch overhead while
/// unblocking quickly when a TB task completes.
const SPIN_BEFORE_BLOCK: u32 = 512;

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
    /// When set, async TB task results are written here instead of `results`.
    /// Provides O(1) sequential-access cache-friendly lookups vs DashMap hash.
    result_ring: Option<Arc<ResultRing>>,
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
            result_ring: self.result_ring.clone(),
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
            result_ring: None,
            deferred_transfers: Vec::with_capacity(MAX_TB_BATCH_SIZE),
            deferred_sequences: Vec::with_capacity(MAX_TB_BATCH_SIZE),
            active_tasks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Configures the handler to write async TB results to `ring` instead of
    /// the `DashMap`. The ring and the pipeline's `result_ring` must share the
    /// same `Arc` so the serve thread can drain them without a map lookup.
    ///
    /// Call this immediately after [`new`][LedgerHandler::new] and before
    /// passing the handler to `PipelineBuilder::add_handler`.
    pub fn with_result_ring(mut self, ring: Arc<ResultRing>) -> Self {
        self.result_ring = Some(ring);
        self
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
        let result_ring_opt = self.result_ring.as_ref().map(Arc::clone);
        let current_seq = sequence;

        // ── Backpressure: cap concurrent in-flight TB tasks ───────────────────
        //
        // Without this cap, if TB RTT grows (DO disk I/O pressure, journal
        // compaction), tasks accumulate faster than they complete. Each task
        // holds a full Transfer Vec (~800 KB). 100 tasks = ~85 MB heap +
        // DashMap with 819,000 entries = severe cache thrashing = serve thread
        // drain stalls = fewer Aeron polls/sec = TPS decay feedback loop.
        //
        // Spin-wait on the DEDICATED runner thread (not a tokio thread — it is
        // safe to block here). The runner stops advancing the ring-buffer
        // gating sequence, which fills the ring buffer, which stalls the serve
        // thread's publish path (it enters the backpressure spin and calls
        // drain), which drains the oldest TB results, which decrements
        // active_tasks here. Self-regulating backpressure.
        {
            let mut spins: u32 = 0;
            while self.active_tasks.load(Ordering::Acquire) >= MAX_CONCURRENT_BATCHES {
                spins = spins.wrapping_add(1);
                if spins & (SPIN_BEFORE_BLOCK - 1) == 0 {
                    std::thread::yield_now();
                } else {
                    std::hint::spin_loop();
                }
            }
        }

        // Track in-flight count. Increment BEFORE spawn so the monitoring
        // counter is never transiently zero between dispatched batches.
        let active_tasks = Arc::clone(&self.active_tasks);
        active_tasks.fetch_add(1, Ordering::Relaxed);
        self.runtime.spawn(async move {
            let tb_t0 = Instant::now();

            // ── Transient-error retry loop ──────────────────────────────────
            //
            // TB returns `LedgerTransient` when the cluster is mid-view-change
            // (node killed, primary rotating).  The Zig client drops the in-
            // flight request and surfaces a transport error.  We must not
            // surface this as a permanent rejection: the transfer may have been
            // committed on the old primary before the interruption (TB's VSR
            // consensus guarantees the commit is either durable or not at all,
            // never half-committed).  Because every transfer carries a unique
            // ID we can retry unconditionally — if TB already committed the
            // batch it returns `Exists*` per-transfer errors which
            // `TigerBeetleClient::create_transfers_batch` normalises back to
            // `Ok(transfer_id)`.
            let tb_results;
            let mut attempt = 0u32;
            loop {
                // Clone the batch for this attempt (Transfer: Clone).
                // We always clone so `all_transfers` remains owned for the next
                // iteration; the final successful call pays one extra clone which
                // is negligible compared to the TB round-trip.
                let batch = all_transfers.clone();
                let results = client.create_transfers_batch(batch).await;

                // A wholesale transient failure means ALL slots are LedgerTransient.
                let all_transient = results.iter().all(|r| {
                    matches!(r, Err(e) if e.is_transient())
                });

                if !all_transient || attempt >= MAX_TRANSIENT_RETRIES {
                    tb_results = results;
                    if attempt > 0 {
                        if all_transient {
                            warn!(
                                attempt,
                                batch_size,
                                "TB transient error persisted after all retries — marking rejected"
                            );
                        } else {
                            info!(
                                attempt,
                                batch_size,
                                "TB batch succeeded after transient retry"
                            );
                        }
                    }
                    break;
                }

                attempt += 1;
                let delay_ms = TRANSIENT_BACKOFF_BASE_MS * (1u64 << attempt.min(4));
                warn!(
                    attempt,
                    delay_ms,
                    batch_size,
                    "TB transient error (view-change?) — backing off before retry"
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }

            let tb_elapsed_ms = tb_t0.elapsed().as_millis();

            // Write deferred results.
            // Committed -> ResultRing (AtomicU8, 256 KB fits in L2 cache).
            // Rejected  -> DashMap fallback (rare; serve thread covers via or_else).
            for (i, seq) in prev_sequences.iter().enumerate() {
                match &tb_results[i] {
                    Ok(transfer_id) => {
                        debug!(sequence = seq, %transfer_id, "LedgerHandler: deferred committed");
                        if let Some(ring) = &result_ring_opt {
                            ring.insert(*seq, *transfer_id);
                        } else {
                            results_map.insert(
                                *seq,
                                TransactionResult::Committed {
                                    transfer_id: *transfer_id,
                                    timestamp: Timestamp::now(),
                                },
                            );
                        }
                    }
                    Err(e) => {
                        debug!(sequence = seq, error = %e, "LedgerHandler: deferred rejected");
                        results_map
                            .insert(*seq, TransactionResult::Rejected { reason: e.clone() });
                    }
                }
            }

            // Write current event result.
            match &tb_results[prev_n] {
                Ok(transfer_id) => {
                    debug!(sequence = current_seq, %transfer_id, "LedgerHandler: current committed");
                    if let Some(ring) = &result_ring_opt {
                        ring.insert(current_seq, *transfer_id);
                    } else {
                        results_map.insert(
                            current_seq,
                            TransactionResult::Committed {
                                transfer_id: *transfer_id,
                                timestamp: Timestamp::now(),
                            },
                        );
                    }
                }
                Err(e) => {
                    debug!(sequence = current_seq, error = %e, "LedgerHandler: current rejected");
                    results_map.insert(
                        current_seq,
                        TransactionResult::Rejected { reason: e.clone() },
                    );
                }
            }

            // Decrement AFTER all DashMap inserts so the runner’s Acquire load
            // of active_tasks < MAX_CONCURRENT_BATCHES implies all results are
            // visible via DashMap’s internal AcqRel shard locking.
            active_tasks.fetch_sub(1, Ordering::Release);

            if tb_elapsed_ms > 5 {
                warn!(
                    batch_size,
                    tb_elapsed_ms, "LedgerHandler: SLOW batch write (>5 ms)"
                );
            } else {
                info!(batch_size, tb_elapsed_ms, "LedgerHandler: batch committed");
            }
        });
    }

    fn clone_handler(&self) -> Box<dyn EventHandler> {
        Box::new(self.clone())
    }
}

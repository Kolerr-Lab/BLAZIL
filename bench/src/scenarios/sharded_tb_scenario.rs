//! Sharded TigerBeetle E2E benchmark.
//!
//! Runs N independent pipelines, each backed by the **same** 3-node
//! TigerBeetle VSR cluster.  Events are submitted directly into the pipeline
//! ring buffers (no transport layer) to isolate pipeline × TigerBeetle
//! throughput from Aeron IPC overhead.
//!
//! With N shards each flushing independent TB batches we saturate the VSR
//! quorum with N × MAX_BATCH_SIZE transfers per RTT:
//!
//! ```text
//! TPS ≈ N_shards × 8_190 / TB_VSR_RTT
//! ```
//!
//! # Usage
//!
//! ```bash
//! BLAZIL_TB_ADDRESS=<ip1>:3000,<ip2>:3001,<ip3>:3002 \
//!   ./target/release/blazil-bench \
//!     --scenario sharded-tb --events 1000000 --shards 2
//! ```
//!
//! Requires `--features tigerbeetle-client`.

#[cfg(feature = "tigerbeetle-client")]
pub mod inner {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_engine::event::{TransactionEvent, TransactionResult};
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::handlers::publish::PublishHandler;
    use blazil_engine::handlers::risk::RiskHandler;
    use blazil_engine::handlers::validation::ValidationHandler;
    use blazil_engine::pipeline::{Pipeline, PipelineBuilder};
    use blazil_engine::result_ring::ResultRing;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::tigerbeetle::TigerBeetleClient;
    use dashmap::DashMap;

    use crate::metrics::BenchmarkResult;

    // ── Constants ─────────────────────────────────────────────────────────────

    /// Ring buffer capacity per shard (power of 2, ≥ 2 × window).
    const CAPACITY_PER_SHARD: usize = 262_144;

    /// Publish window per shard: max in-flight events before draining.
    /// Mirrors WINDOW_SIZE_TB from aeron_scenario for fair comparison.
    const WINDOW_PER_SHARD: usize = 131_072;

    /// Max transfer amount (100 billion minor units).
    const MAX_AMOUNT_UNITS: u64 = 100_000_000_000;

    /// Warmup events per shard — enough to prime TB batching + JIT.
    const WARMUP_PER_SHARD: u64 = 2_000;

    /// Spin hint count on ring-full backpressure before yielding.
    const BACKPRESSURE_SPIN: usize = 64;

    // ── Shard state passed into each producer thread ──────────────────────────

    struct ShardContext {
        pipeline: Pipeline,
        result_ring: Arc<ResultRing>,
        results: Arc<DashMap<i64, TransactionResult>>,
        debit_id: AccountId,
        credit_id: AccountId,
    }

    // ── Public entry point ────────────────────────────────────────────────────

    /// Run the sharded TigerBeetle E2E benchmark.
    ///
    /// * `events`      — total events across all shards (divided equally)
    /// * `shard_count` — number of independent pipeline shards (power of 2)
    ///
    /// Panics if `BLAZIL_TB_ADDRESS` is not set or TB connection fails.
    pub async fn run(
        events: u64,
        shard_count: usize,
        duration_secs: Option<u64>,
    ) -> BenchmarkResult {
        assert!(
            shard_count.is_power_of_two() && shard_count >= 1,
            "shard_count must be a power of 2, got {shard_count}"
        );

        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS")
            .expect("BLAZIL_TB_ADDRESS must be set for --scenario sharded-tb");

        let events_per_shard = events / shard_count as u64;
        let total_events = events_per_shard * shard_count as u64;
        let duration_mode = duration_secs.is_some();

        let usd = parse_currency("USD").expect("USD currency");

        println!("Scenario      : sharded-tb");
        println!("Shards        : {shard_count}");
        if let Some(dur) = duration_secs {
            println!("Mode          : time-based ({dur}s)");
        } else {
            println!("Events/shard  : {events_per_shard}");
            println!("Total events  : {total_events}");
        }
        println!("Ledger        : TigerBeetle @ {tb_addr}");
        println!("Capacity/shard: {CAPACITY_PER_SHARD}");
        println!("Window/shard  : {WINDOW_PER_SHARD}");

        // ── Shared ledger runtime ─────────────────────────────────────────────
        // VSR is I/O-bound: 2 workers per 4 shards is sufficient.
        // Cap raised to 16 for ≥8-shard runs (v0.3 scaling tests).
        let rt_workers = (shard_count / 2).clamp(2, 16);
        let ledger_rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(rt_workers)
                .thread_name("blazil-ledger-rt")
                .enable_all()
                .build()
                .expect("ledger runtime"),
        );

        // ── Connect to TigerBeetle ────────────────────────────────────────────
        println!("[diag] connecting to TigerBeetle @ {tb_addr}...");
        let tb_client = Arc::new(
            TigerBeetleClient::connect(&tb_addr, 0)
                .await
                .expect("TigerBeetle connect"),
        );
        println!("[diag] TB connect OK");

        // ── Build N shards ────────────────────────────────────────────────────
        let mut shard_contexts: Vec<ShardContext> = Vec::with_capacity(shard_count);

        for shard_id in 0..shard_count {
            // One debit + one credit account per shard to avoid cross-shard
            // TB balance contention.
            let debit_id = tb_client
                .create_account(Account::new(
                    AccountId::new(),
                    LedgerId::USD,
                    usd,
                    1,
                    AccountFlags::default(),
                ))
                .await
                .unwrap_or_else(|e| panic!("shard {shard_id} debit account: {e}"));

            let credit_id = tb_client
                .create_account(Account::new(
                    AccountId::new(),
                    LedgerId::USD,
                    usd,
                    1,
                    AccountFlags::default(),
                ))
                .await
                .unwrap_or_else(|e| panic!("shard {shard_id} credit account: {e}"));

            println!("[diag] shard {shard_id} accounts: debit={debit_id} credit={credit_id}");

            // Pipeline builder for this shard.
            let builder = PipelineBuilder::new()
                .with_capacity(CAPACITY_PER_SHARD)
                .with_global_shard_id(shard_id);

            let results = builder.results();
            let result_ring = builder.result_ring();

            let ledger_handler = LedgerHandler::new(
                Arc::clone(&tb_client),
                Arc::clone(&ledger_rt),
                Arc::clone(&results),
            )
            .with_result_ring(Arc::clone(&result_ring));

            let (pipeline, runners) = builder
                .add_handler(ValidationHandler::new(Arc::clone(&results)))
                .add_handler(RiskHandler::new(MAX_AMOUNT_UNITS, Arc::clone(&results)))
                .add_handler(ledger_handler)
                .add_handler(PublishHandler::new(Arc::clone(&results)))
                .build()
                .unwrap_or_else(|e| panic!("shard {shard_id} pipeline build: {e}"));

            // Spawn consumer thread(s) for this shard.
            // JoinHandles are dropped (detached) — the threads run until the
            // process exits.  For a finite bench run this is intentional.
            for runner in runners {
                let _ = runner.run();
            }

            shard_contexts.push(ShardContext {
                pipeline,
                result_ring,
                results,
                debit_id,
                credit_id,
            });
        }
        println!("[diag] {shard_count} shard pipeline(s) started");

        // ── Warmup ────────────────────────────────────────────────────────────
        println!("[diag] warmup ({WARMUP_PER_SHARD} events/shard)...");
        for ctx in &shard_contexts {
            for _ in 0..WARMUP_PER_SHARD {
                let event = make_event(ctx.debit_id, ctx.credit_id);
                // Discard returned sequence — warmup results are not tracked.
                publish_with_backpressure(&ctx.pipeline, event);
            }
        }
        // Give TB time to drain the warmup batches before the timed section.
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        println!("[diag] warmup done — starting timed bench");

        // ── Timed bench ───────────────────────────────────────────────────────
        // One producer OS thread per shard for maximum parallelism.
        // Each thread's VecDeque<(seq, Instant)> holds in-flight (seq, send_time)
        // pairs.  They are drained in FIFO order via result_ring.try_remove(seq).
        let committed_total = Arc::new(AtomicU64::new(0));
        let rejected_total = Arc::new(AtomicU64::new(0));

        // Duration-mode: a background thread sets stop_flag after `dur` seconds.
        // Event-mode: stop_flag is never set — threads exit via `received >= n`.
        let stop_flag = Arc::new(AtomicBool::new(false));
        if let Some(dur) = duration_secs {
            let flag = Arc::clone(&stop_flag);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(dur));
                flag.store(true, Ordering::Relaxed);
                println!("[diag] duration elapsed — signalling shard threads to drain and exit");
            });
        }

        let wall_start = Instant::now();

        let mut producer_handles = Vec::with_capacity(shard_count);

        for (shard_id, ctx) in shard_contexts.into_iter().enumerate() {
            let ShardContext {
                pipeline,
                result_ring,
                results,
                debit_id,
                credit_id,
            } = ctx;

            let n = events_per_shard;
            let committed_total = Arc::clone(&committed_total);
            let rejected_total = Arc::clone(&rejected_total);
            let stop_flag = Arc::clone(&stop_flag);

            let handle = std::thread::Builder::new()
                .name(format!("bench-shard-{shard_id}"))
                .spawn(move || {
                    let mut latencies: Vec<u64> = if duration_mode {
                        Vec::new() // capacity unknown in time-based mode
                    } else {
                        Vec::with_capacity(n as usize)
                    };
                    // VecDeque of (ring_seq, send_time) for in-flight events.
                    let mut in_flight: VecDeque<(i64, Instant)> =
                        VecDeque::with_capacity(WINDOW_PER_SHARD);
                    let mut sent = 0u64;
                    let mut received = 0u64;
                    let mut committed = 0u64;
                    let mut rejected = 0u64;
                    let mut last_hb = Instant::now();

                    // Pre-compute label once to avoid temp-borrow in format string.
                    let total_label: String = if duration_mode {
                        "\u{221e}".to_string()
                    } else {
                        n.to_string()
                    };

                    // Fill initial window.
                    // Duration-mode always fills the full WINDOW; event-mode caps at n.
                    let initial = if duration_mode {
                        WINDOW_PER_SHARD
                    } else {
                        WINDOW_PER_SHARD.min(n as usize)
                    };
                    for _ in 0..initial {
                        let event = make_event(debit_id, credit_id);
                        let seq = publish_with_backpressure(&pipeline, event);
                        in_flight.push_back((seq, Instant::now()));
                        sent += 1;
                    }

                    // Main drain loop.
                    // Event-mode:    exits when received >= n.
                    // Duration-mode: stops sending when stop_flag fires, then
                    //                drains remaining in_flight before exiting.
                    loop {
                        let done = if duration_mode {
                            stop_flag.load(Ordering::Relaxed) && in_flight.is_empty()
                        } else {
                            received >= n
                        };
                        if done {
                            break;
                        }

                        // Try to drain front of in_flight.
                        let mut drained = false;
                        if let Some(&(seq, t0)) = in_flight.front() {
                            // Hot path: committed result in ResultRing.
                            if result_ring.try_remove(seq).is_some() {
                                latencies.push(t0.elapsed().as_nanos() as u64);
                                committed += 1;
                                in_flight.pop_front();
                                received += 1;
                                drained = true;
                            // Cold path: rejected result in DashMap.
                            } else if results.remove(&seq).is_some() {
                                latencies.push(t0.elapsed().as_nanos() as u64);
                                rejected += 1;
                                in_flight.pop_front();
                                received += 1;
                                drained = true;
                            }
                        }

                        // Refill window while still in the send phase.
                        let can_send = if duration_mode {
                            !stop_flag.load(Ordering::Relaxed)
                        } else {
                            sent < n
                        };
                        if drained && can_send && in_flight.len() < WINDOW_PER_SHARD {
                            let event = make_event(debit_id, credit_id);
                            let seq = publish_with_backpressure(&pipeline, event);
                            in_flight.push_back((seq, Instant::now()));
                            sent += 1;
                        }

                        if !drained {
                            // No result ready yet — hint the CPU.
                            if last_hb.elapsed().as_secs() >= 5 {
                                // Periodic DashMap drain: evict entries whose
                                // sequences predate the current in_flight window.
                                // Cleans warmup residue + stale failover rejections.
                                if let Some(&(min_seq, _)) = in_flight.front() {
                                    results.retain(|&seq, _| seq >= min_seq);
                                }
                                println!(
                                    "[shard {shard_id}] recv={received}/{total_label} \
                                     sent={sent} inflight={} results_map={}",
                                    in_flight.len(),
                                    results.len(),
                                );
                                last_hb = Instant::now();
                            }
                            for _ in 0..8 {
                                std::hint::spin_loop();
                            }
                        }
                    }

                    committed_total.fetch_add(committed, Ordering::Relaxed);
                    rejected_total.fetch_add(rejected, Ordering::Relaxed);
                    latencies
                })
                .unwrap_or_else(|e| panic!("shard {shard_id} thread spawn: {e}"));

            producer_handles.push(handle);
        }

        // Wait for all producer threads to finish.
        let latency_cap = if duration_mode {
            0
        } else {
            total_events as usize
        };
        let mut all_latencies: Vec<u64> = Vec::with_capacity(latency_cap);
        for handle in producer_handles {
            let lats = handle.join().expect("producer thread panicked");
            all_latencies.extend(lats);
        }

        let wall_duration = wall_start.elapsed();

        let committed = committed_total.load(Ordering::Relaxed);
        let rejected = rejected_total.load(Ordering::Relaxed);
        let actual_total = committed + rejected;
        let error_rate = if actual_total > 0 {
            rejected as f64 / actual_total as f64 * 100.0
        } else {
            0.0
        };

        println!("Committed : {committed} / Rejected : {rejected} / Error rate : {error_rate:.2}%");

        // In duration-mode the pre-computed total_events may differ from what
        // was actually processed; use the real count for the result record.
        let total_for_result = if duration_mode {
            actual_total
        } else {
            total_events
        };
        BenchmarkResult::new(
            &format!("Sharded TB E2E ({shard_count} shards)"),
            total_for_result,
            wall_duration,
            &mut all_latencies,
        )
        .with_counts(committed, rejected)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Creates a single-transfer event (fresh TransactionId each call).
    fn make_event(debit_id: AccountId, credit_id: AccountId) -> TransactionEvent {
        TransactionEvent::new(
            TransactionId::new(),
            debit_id,
            credit_id,
            1_00_u64, // 1.00 USD in minor units
            LedgerId::USD,
            1,
        )
    }

    /// Publishes `event` into the pipeline, spinning on ring-full backpressure.
    ///
    /// Returns the assigned ring buffer sequence number so the caller can
    /// correlate it with `ResultRing::try_remove(seq)` later.
    ///
    /// # Single-producer safety
    ///
    /// We check `has_available_capacity()` before moving `event` into
    /// `publish_event`.  Since each pipeline shard has exactly one producer
    /// thread, capacity can only increase between the check and the call
    /// (the consumer frees slots; capacity can never decrease from our side).
    fn publish_with_backpressure(pipeline: &Pipeline, event: TransactionEvent) -> i64 {
        loop {
            if pipeline.ring_buffer().has_available_capacity() {
                return pipeline
                    .publish_event(event)
                    .expect("capacity pre-checked; single producer");
            }
            for _ in 0..BACKPRESSURE_SPIN {
                std::hint::spin_loop();
            }
            std::thread::yield_now();
        }
    }
}

#[cfg(feature = "tigerbeetle-client")]
pub use inner::run;

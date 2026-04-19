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
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[cfg(feature = "metrics-ws")]
    use crate::ws_server::ConfigCache;
    use tokio::sync::broadcast;

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
    const CAPACITY_PER_SHARD: usize = 2_048;

    /// Publish window per shard: max in-flight events before draining.
    /// 1024 keeps P99 latency low while providing enough pipeline
    /// depth for peak TPS on 4 shards against a 3-node VSR cluster.
    const WINDOW_PER_SHARD: usize = 1_024;

    /// Max transfer amount (100 billion minor units).
    const MAX_AMOUNT_UNITS: u64 = 100_000_000_000;

    /// Warmup events per shard — enough to prime TB batching + JIT.
    const WARMUP_PER_SHARD: u64 = 500;

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
    /// * `events`        — total events (divided equally across shards)
    /// * `shard_count`   — number of independent pipeline shards (power of 2)
    /// * `duration_secs` — if Some(N), run for N seconds instead of fixed events
    /// * `metrics_tx`    — optional broadcast channel for live dashboard metrics
    ///
    /// Panics if `BLAZIL_TB_ADDRESS` is not set or TB connection fails.
    pub async fn run(
        events: u64,
        shard_count: usize,
        duration_secs: Option<u64>,
        metrics_tx: Option<broadcast::Sender<String>>,
        #[cfg(feature = "metrics-ws")] config_cache: Option<ConfigCache>,
        #[cfg(not(feature = "metrics-ws"))] _config_cache: Option<()>,
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

        // ── Live dashboard metrics config ─────────────────────────────────────
        let dur_json = duration_secs
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let rt_workers_hint = (shard_count / 2).clamp(2, 32);
        if let Some(ref tx) = metrics_tx {
            let msg = format!(
                "{{\"type\":\"config\",\"shards\":{shard_count},\
\"duration_secs\":{dur_json},\"rt_workers\":{rt_workers_hint},\
\"tb_addr\":\"{tb_addr}\",\"capacity_per_shard\":{CAPACITY_PER_SHARD},\
\"window_per_shard\":{WINDOW_PER_SHARD}}}"
            );
            // Store in config_cache so clients connecting after bench start
            // still receive this message and transition to "running".
            #[cfg(feature = "metrics-ws")]
            if let Some(ref cache) = config_cache {
                *cache.write().await = Some(msg.clone());
            }
            tx.send(msg).ok();
        }

        // ── Shared TB client for account creation only ────────────────────────
        // Each shard gets its own dedicated TB client + ledger runtime below.
        // This shared client is only used to create accounts before bench start.
        println!("[diag] connecting to TigerBeetle @ {tb_addr} (setup client)...");
        let setup_client = Arc::new(
            TigerBeetleClient::connect(&tb_addr, 0)
                .await
                .expect("TigerBeetle connect"),
        );
        println!("[diag] TB connect OK");

        // ── Build N shards — each with dedicated TB client + ledger runtime ──
        let mut shard_contexts: Vec<ShardContext> = Vec::with_capacity(shard_count);

        for shard_id in 0..shard_count {
            // One debit + one credit account per shard to avoid cross-shard
            // TB balance contention.
            let debit_id = setup_client
                .create_account(Account::new(
                    AccountId::new(),
                    LedgerId::USD,
                    usd,
                    1,
                    AccountFlags::default(),
                ))
                .await
                .unwrap_or_else(|e| panic!("shard {shard_id} debit account: {e}"));

            let credit_id = setup_client
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

            // Dedicated TB client per shard — no cross-shard VSR queue contention.
            let tb_client = Arc::new(
                TigerBeetleClient::connect(&tb_addr, 0)
                    .await
                    .unwrap_or_else(|e| panic!("shard {shard_id} TB client: {e}")),
            );

            // Dedicated ledger runtime per shard — 2 workers each.
            // Shared runtime caused task starvation at high concurrency
            // (4 shards × 8 concurrent batches = 32 tasks competing for
            // shard_count/2 shared workers → deadlock at ~t=13s).
            let ledger_rt = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name(format!("blazil-ledger-rt-{shard_id}"))
                    .enable_all()
                    .build()
                    .unwrap_or_else(|e| panic!("shard {shard_id} ledger runtime: {e}")),
            );

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
        // 200 events/shard — warms TB's LSM/memtable so first timed second
        // doesn't pay cold-start penalty. 200 << CAPACITY_PER_SHARD=2048 so
        // the ring never fills and this loop completes without spinning.
        println!("[diag] warmup (2000 events/shard)...");
        for ctx in &shard_contexts {
            for _ in 0..2_000u64 {
                let event = make_event(ctx.debit_id, ctx.credit_id);
                publish_with_backpressure(&ctx.pipeline, event);
            }
        }
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        println!("[diag] warmup done — starting timed bench");

        // Broadcast bench_start event to dashboard.
        if let Some(ref tx) = metrics_tx {
            tx.send(
                "{\"type\":\"event\",\"t\":0,\"kind\":\"bench_start\",\
\"message\":\"Warmup complete — timed bench running\"}"
                    .to_string(),
            )
            .ok();
        }

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
        // Capture start as Duration-from-epoch so shard threads can compute
        // wall-clock second buckets without a shared reference to Instant.
        let wall_start_for_shards = wall_start;

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
            let metrics_tx_shard = metrics_tx.clone();
            let shard_wall_start = wall_start_for_shards;

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

                    // Per-second TPS tracking — keyed by wall-clock second so
                    // all shards share the same bucket timeline regardless of
                    // thread scheduling jitter.
                    let mut per_second_tps: Vec<(u64, u64)> = Vec::new();
                    let mut last_window_time = Instant::now();
                    let mut last_window_received = 0u64;

                    // Rolling latency window — 512 samples sorted each second
                    // for live p50/p99 estimates in the dashboard.
                    let mut rolling_lat: VecDeque<u64> = VecDeque::with_capacity(512);

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
                    let mut drain_deadline: Option<Instant> = None;
                    loop {
                        let stopped = duration_mode && stop_flag.load(Ordering::Relaxed);
                        // Start a 5s drain deadline the moment stop_flag fires.
                        if stopped && drain_deadline.is_none() {
                            drain_deadline = Some(Instant::now() + Duration::from_secs(5));
                        }
                        let timed_out = drain_deadline.map_or(false, |d| Instant::now() >= d);
                        let done = if duration_mode {
                            (stopped && in_flight.is_empty()) || timed_out
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
                                let lat = t0.elapsed().as_nanos() as u64;
                                latencies.push(lat);
                                if rolling_lat.len() >= 512 {
                                    rolling_lat.pop_front();
                                }
                                rolling_lat.push_back(lat);
                                committed += 1;
                                in_flight.pop_front();
                                received += 1;
                                drained = true;
                            // Cold path: rejected result in DashMap.
                            } else if results.remove(&seq).is_some() {
                                let lat = t0.elapsed().as_nanos() as u64;
                                latencies.push(lat);
                                if rolling_lat.len() >= 512 {
                                    rolling_lat.pop_front();
                                }
                                rolling_lat.push_back(lat);
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

                        // Per-second TPS window: capture throughput every 1s.
                        let elapsed_secs = last_window_time.elapsed().as_secs();
                        if elapsed_secs >= 1 {
                            let delta_received = received.saturating_sub(last_window_received);
                            // Use wall-clock second so all shards align to the
                            // same bucket even if threads started at different times.
                            let current_second = shard_wall_start.elapsed().as_secs();
                            per_second_tps.push((current_second, delta_received));
                            last_window_time = Instant::now();
                            last_window_received = received;

                            // Live p50/p99 from rolling window (512-sample sort ~3µs).
                            let (p50_us, p99_us) = if rolling_lat.len() >= 4 {
                                let mut s: Vec<u64> = rolling_lat.iter().copied().collect();
                                s.sort_unstable();
                                (s[s.len() / 2] / 1_000, s[(s.len() * 99) / 100] / 1_000)
                            } else {
                                (0u64, 0u64)
                            };

                            // Print per-second TPS to stdout so progress is
                            // visible without the dashboard.
                            println!(
                                "[t+{current_second:3}s] shard={shard_id} \
                                 TPS={delta_received:>8} inflight={:>4} \
                                 p99={p99_us}µs",
                                in_flight.len(),
                            );

                            // Broadcast per-shard tick to live dashboard.
                            if let Some(ref tx) = metrics_tx_shard {
                                let fl = in_flight.len();
                                let msg = format!(
                                    "{{\"type\":\"tick\",\"t\":{current_second},\
\"shard_id\":{shard_id},\"tps\":{delta_received},\
\"committed_total\":{committed},\"rejected_total\":{rejected},\
\"inflight\":{fl},\"sent_total\":{sent},\
\"p50_us\":{p50_us},\"p99_us\":{p99_us}}}"
                                );
                                tx.send(msg).ok();
                            }
                        }

                        if !drained {
                            // No result ready yet — hint the CPU.
                            if last_hb.elapsed().as_secs() >= 2 {
                                // Periodic DashMap drain: evict entries whose
                                // sequences predate the current in_flight window.
                                if let Some(&(min_seq, _)) = in_flight.front() {
                                    results.retain(|&seq, _| seq >= min_seq);
                                }
                                println!(
                                    "[HB shard={shard_id}] recv={received}/{total_label} \
                                     sent={sent} inflight={} results_map={}",
                                    in_flight.len(),
                                    results.len(),
                                );
                                // Early abort: if pipeline is completely dead
                                // (zero results in first 10s), break immediately.
                                if shard_wall_start.elapsed().as_secs() > 10 && received == 0 {
                                    println!(
                                        "[FATAL shard={shard_id}] No results after 10s — \
                                         pipeline stall. Check TB logs: \
                                         tail /tmp/tb0.log /tmp/tb1.log /tmp/tb2.log"
                                    );
                                    break;
                                }
                                last_hb = Instant::now();
                            }
                            for _ in 0..8 {
                                std::hint::spin_loop();
                            }
                        }
                    }

                    // Capture final partial second only if it is a full 1-second
                    // window; a short partial second would drag down min_tps and
                    // make consistency appear artificially low.
                    let elapsed_secs = last_window_time.elapsed().as_secs();
                    if elapsed_secs >= 1 {
                        let delta_received = received.saturating_sub(last_window_received);
                        let current_second = per_second_tps.len() as u64;
                        per_second_tps.push((current_second, delta_received));
                    }

                    committed_total.fetch_add(committed, Ordering::Relaxed);
                    rejected_total.fetch_add(rejected, Ordering::Relaxed);
                    (latencies, per_second_tps)
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
        let mut windowed_tps: BTreeMap<u64, u64> = BTreeMap::new();

        for handle in producer_handles {
            let (lats, per_sec) = handle.join().expect("producer thread panicked");
            all_latencies.extend(lats);
            // Aggregate per-second TPS across shards.
            for (sec, tps) in per_sec {
                *windowed_tps.entry(sec).or_insert(0) += tps;
            }
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

        // Print per-second TPS breakdown for failover analysis.
        if !windowed_tps.is_empty() {
            println!("\n[per-second TPS breakdown]");
            for (sec, tps) in &windowed_tps {
                println!("  t+{:3}s: {:>9} TPS", sec, tps);
            }
            let avg_tps = if !windowed_tps.is_empty() {
                windowed_tps.values().sum::<u64>() / windowed_tps.len() as u64
            } else {
                0
            };
            let max_tps = windowed_tps.values().max().copied().unwrap_or(0);
            let min_tps = windowed_tps.values().min().copied().unwrap_or(0);
            println!(
                "  avg: {} TPS | max: {} TPS | min: {} TPS\n",
                avg_tps, max_tps, min_tps
            );
        }

        // In duration-mode the pre-computed total_events may differ from what
        // was actually processed; use the real count for the result record.
        let total_for_result = if duration_mode {
            actual_total
        } else {
            total_events
        };
        let result = BenchmarkResult::new(
            &format!("Sharded TB E2E ({shard_count} shards)"),
            total_for_result,
            wall_duration,
            &mut all_latencies,
        )
        .with_counts(committed, rejected);

        // Broadcast final summary to dashboard.
        if let Some(ref tx) = metrics_tx {
            let survival_rate = if actual_total > 0 {
                committed as f64 / actual_total as f64 * 100.0
            } else {
                100.0
            };
            let tps_vals: Vec<u64> = windowed_tps.values().copied().collect();
            let avg_tps = if !tps_vals.is_empty() {
                tps_vals.iter().sum::<u64>() / tps_vals.len() as u64
            } else {
                result.tps
            };
            let max_tps = tps_vals.iter().max().copied().unwrap_or(result.tps);
            // Use result.tps as fallback for min too — if windowed_tps is empty
            // (bench finished in < 1s), both min and max should equal the
            // overall TPS so consistency reports 100% rather than 0%.
            let min_tps = tps_vals.iter().min().copied().unwrap_or(result.tps);
            let consistency = if max_tps > 0 {
                min_tps as f64 / max_tps as f64 * 100.0
            } else {
                0.0
            };
            let wall_secs = wall_duration.as_secs_f64();
            let msg = format!(
                "{{\"type\":\"summary\",\
\"total_committed\":{committed},\"total_rejected\":{rejected},\
\"error_rate\":{error_rate:.4},\"survival_rate\":{survival_rate:.4},\
\"tps\":{},\"avg_tps\":{avg_tps},\"max_tps\":{max_tps},\"min_tps\":{min_tps},\
\"consistency\":{consistency:.2},\
\"p50_ns\":{},\"p99_ns\":{},\"p999_ns\":{},\
\"mean_ns\":{},\"wall_secs\":{wall_secs:.3},\"shards\":{shard_count}}}",
                result.tps, result.p50_ns, result.p99_ns, result.p99_9_ns, result.mean_ns,
            );
            tx.send(msg).ok();
        }

        result
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

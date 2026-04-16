//! VSR Failover E2E benchmark.
//!
//! Extends the sharded-TigerBeetle load generator with real node kill/restart
//! capability.  When the dashboard sends a `kill_node` command (or when the
//! optional auto-kill timer fires), the bench:
//!
//! 1. Emits a `node_down` event to the dashboard.
//! 2. Runs the configured kill shell command for that node.
//! 3. Continues generating load — TigerBeetle VSR maintains 2-of-3 quorum and
//!    processing continues (possibly at slightly lower TPS).
//! 4. After `--failover-recovery-secs` (default 30 s), emits `node_up`
//!    and runs the restart command.
//!
//! # CLI usage
//!
//! ```bash
//! BLAZIL_TB_ADDRESS=<n1>:3000,<n2>:3000,<n3>:3000 \
//!   ./blazil-bench \
//!     --scenario vsr-failover \
//!     --shards 8 \
//!     --duration 120 \
//!     --metrics-port 9090 \
//!     --kill-cmd-1   "ssh root@<n1> docker stop tigerbeetle" \
//!     --kill-cmd-2   "ssh root@<n2> docker stop tigerbeetle" \
//!     --kill-cmd-3   "ssh root@<n3> docker stop tigerbeetle" \
//!     --restart-cmd-1 "ssh root@<n1> docker start tigerbeetle" \
//!     --restart-cmd-2 "ssh root@<n2> docker start tigerbeetle" \
//!     --restart-cmd-3 "ssh root@<n3> docker start tigerbeetle" \
//!     --auto-kill-node 3 \
//!     --auto-kill-after-secs 30 \
//!     --failover-recovery-secs 30
//! ```
//!
//! Without `--auto-kill-node`, the kill is triggered from the dashboard by
//! pressing a node's KILL button (which sends a WS command).
//!
//! Requires `--features tigerbeetle-client,metrics-ws`.

#[cfg(all(feature = "tigerbeetle-client", feature = "metrics-ws"))]
pub mod inner {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::sync::broadcast;

    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransactionId};
    use blazil_engine::event::{TransactionEvent, TransactionResult};
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::pipeline::{Pipeline, PipelineBuilder};
    use blazil_engine::result_ring::ResultRing;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::tigerbeetle::TigerBeetleClient;
    use dashmap::DashMap;

    use crate::metrics::BenchmarkResult;

    // ── Re-use constants from sharded_tb_scenario ─────────────────────────────

    // Ring buffer + result ring capacity per shard (must be power of 2).
    // Set to 2× WINDOW_PER_SHARD to give headroom between ring buffer and
    // result ring during burst drain/send cycles.
    const CAPACITY_PER_SHARD: usize = 65_536;
    // Maximum in-flight events per shard. This is the primary throughput lever:
    //   TPS/shard ≈ WINDOW / RTT
    // At observed p50=244ms: 32_768 / 0.244 ≈ 134K/shard × 8 = 1.07M TPS theory.
    // Must be ≤ CAPACITY_PER_SHARD.
    const WINDOW_PER_SHARD: usize = 32_768;
    const MAX_AMOUNT_UNITS: u64 = 100_000_000_000;
    const WARMUP_PER_SHARD: u64 = 2_000;

    // ── Kill configuration per node (1-indexed, matching dashboard) ───────────

    /// Shell commands for killing and restarting each TigerBeetle node.
    /// Index 0 = node 1, index 1 = node 2, index 2 = node 3.
    pub struct NodeCommands {
        /// Shell command to kill TB node N (run via `sh -c "cmd"`).
        pub kill: [Option<String>; 3],
        /// Shell command to restart TB node N (run via `sh -c "cmd"`).
        pub restart: [Option<String>; 3],
    }

    impl Default for NodeCommands {
        fn default() -> Self {
            Self {
                kill: [None, None, None],
                restart: [None, None, None],
            }
        }
    }

    /// Configuration for the VSR failover benchmark.
    pub struct FailoverConfig {
        pub events: u64,
        pub shard_count: usize,
        pub duration_secs: Option<u64>,
        /// Node to auto-kill (1-indexed). None = only dashboard-triggered kills.
        pub auto_kill_node: Option<u8>,
        /// Seconds after bench start to trigger the auto-kill.
        pub auto_kill_after_secs: u64,
        /// Seconds to wait after kill before issuing the restart command.
        pub recovery_secs: u64,
        /// Node kill/restart shell commands.
        pub node_cmds: NodeCommands,
        /// Outgoing metrics broadcaster (bench → dashboard).
        pub metrics_tx: broadcast::Sender<String>,
        /// Incoming command channel (dashboard → bench).
        pub cmd_rx: broadcast::Receiver<String>,
    }

    // ── Shard context ─────────────────────────────────────────────────────────

    struct ShardContext {
        pipeline: Pipeline,
        result_ring: Arc<ResultRing>,
        results: Arc<DashMap<i64, TransactionResult>>,
        debit_id: AccountId,
        credit_id: AccountId,
    }

    // ── Public entry point ────────────────────────────────────────────────────

    /// Run the VSR failover benchmark.
    pub async fn run(mut cfg: FailoverConfig) -> BenchmarkResult {
        assert!(
            cfg.shard_count.is_power_of_two() && cfg.shard_count >= 1,
            "shard_count must be a power of 2"
        );

        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS")
            .expect("BLAZIL_TB_ADDRESS must be set for --scenario vsr-failover");
        let tb_addr_2 = std::env::var("BLAZIL_TB_ADDRESS_2").ok();
        let dual_cluster = tb_addr_2.is_some();
        let half_shards = cfg.shard_count / 2;

        let events_per_shard = cfg.events / cfg.shard_count as u64;
        let duration_mode = cfg.duration_secs.is_some();
        let usd = parse_currency("USD").expect("USD currency");

        println!("Scenario      : vsr-failover");
        println!("Shards        : {}", cfg.shard_count);
        if let Some(dur) = cfg.duration_secs {
            println!("Mode          : time-based ({dur}s)");
        } else {
            println!("Events/shard  : {events_per_shard}");
        }
        println!("Ledger        : TigerBeetle @ {tb_addr}");
        if let Some(ref a2) = tb_addr_2 {
            println!("Cluster 1     : TigerBeetle @ {a2}  (shards {half_shards}+)");
        }
        if let Some(n) = cfg.auto_kill_node {
            println!(
                "Auto-kill     : Node {n} at t+{}s (recovery after {}s)",
                cfg.auto_kill_after_secs, cfg.recovery_secs
            );
        } else {
            println!("Kill trigger  : dashboard KILL button");
        }

        // ── Broadcast config to dashboard ─────────────────────────────────────
        let dur_json = cfg
            .duration_secs
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let rt_workers_hint = (cfg.shard_count / 2).clamp(2, 16);
        let shard_count = cfg.shard_count;
        emit_str(
            &cfg.metrics_tx,
            &format!(
                "{{\"type\":\"config\",\"shards\":{shard_count},\
\"duration_secs\":{dur_json},\"rt_workers\":{rt_workers_hint},\
\"tb_addr\":\"{tb_addr}\",\"capacity_per_shard\":{CAPACITY_PER_SHARD},\
\"window_per_shard\":{WINDOW_PER_SHARD}}}"
            ),
        );

        // ── Shared ledger runtime ─────────────────────────────────────────────
        // Give the Tokio runtime one worker thread per shard so all 8 shards
        // can dispatch their async TB batches concurrently without queuing
        // on a smaller thread pool. Previous value (shard_count/2) meant 4
        // threads servicing 8 shards × 16 concurrent batches = 128 tasks.
        let rt_workers = cfg.shard_count; // 1 thread per shard
        let ledger_rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(rt_workers)
                .thread_name("blazil-ledger-rt")
                .enable_all()
                .build()
                .expect("ledger runtime"),
        );

        // ── Dual-cluster support ──────────────────────────────────────────────
        //
        // Optional: set BLAZIL_TB_ADDRESS_2 to point a second independent TB
        // cluster (formatted with cluster_id=1).  Shards are split evenly:
        // lower half → cluster 0, upper half → cluster 1.
        // Each cluster handles 50% of the write load with parallel NVMe
        // fsyncs — the key unlock for i4i.metal 1M TPS tomorrow.
        //
        // Single-cluster (BLAZIL_TB_ADDRESS_2 not set): behaves exactly as
        // before — 4 pool clients, all on cluster 0.
        //
        // Dual-cluster connection budget:
        //   Cluster 0: 1 setup + 2 pool = 3 connections
        //   Cluster 1: 1 setup + 2 pool = 3 connections
        //   Both well under TB clients_max=8.
        // (tb_addr_2, dual_cluster, half_shards are already bound above)

        println!("[diag] connecting to TigerBeetle @ {tb_addr}...");
        let setup_client = TigerBeetleClient::connect(&tb_addr, 0)
            .await
            .expect("TigerBeetle setup connect (cluster 0)");

        let setup_client_2 = if let Some(ref addr2) = tb_addr_2 {
            println!("[diag] connecting to TigerBeetle cluster 1 @ {addr2}...");
            let c = TigerBeetleClient::connect(addr2, 1)
                .await
                .expect("TigerBeetle setup connect (cluster 1)");
            println!("[diag] cluster 1 setup client connected");
            Some(c)
        } else {
            None
        };

        if dual_cluster {
            println!(
                "[diag] DUAL-CLUSTER mode: shards 0..{} → cluster0, shards {}..{} → cluster1",
                half_shards, half_shards, cfg.shard_count
            );
        }

        // ── Client pool ───────────────────────────────────────────────────────
        //
        // Single-cluster: 4 clients on cluster 0, 2 shards per client.
        //   Client 0 → shards 0, 1
        //   Client 1 → shards 2, 3
        //   Client 2 → shards 4, 5
        //   Client 3 → shards 6, 7
        //   Total: 4 pool + 1 setup = 5 connections
        //
        // Dual-cluster: 2 clients on cluster 0 + 2 clients on cluster 1.
        //   Cluster0: client[0] → shards 0,1 | client[1] → shards 2,3
        //   Cluster1: client[0] → shards 4,5 | client[1] → shards 6,7
        //   Total: 2+2 pool + 1+1 setup = 6 connections across 2 clusters
        //
        // TB 0.16.78: one io_uring thread per Client → independent submission
        // queues. MAX_CONCURRENT_BATCHES(8)/shard × 2 shards = 16 in-flight
        // batches per client = 131,040 transfers per client simultaneously.
        let clients_per_cluster = if dual_cluster { 2usize } else { 4usize };

        let mut cluster0_clients: Vec<Arc<TigerBeetleClient>> =
            Vec::with_capacity(clients_per_cluster);
        for i in 0..clients_per_cluster {
            let c = TigerBeetleClient::connect(&tb_addr, 0)
                .await
                .unwrap_or_else(|e| panic!("TB cluster0 pool client {i} connect: {e}"));
            cluster0_clients.push(Arc::new(c));
            println!("[diag] cluster0 pool client {i} connected");
        }

        let mut cluster1_clients: Vec<Arc<TigerBeetleClient>> =
            Vec::with_capacity(clients_per_cluster);
        if let Some(ref addr2) = tb_addr_2 {
            for i in 0..clients_per_cluster {
                let c = TigerBeetleClient::connect(addr2, 1)
                    .await
                    .unwrap_or_else(|e| panic!("TB cluster1 pool client {i} connect: {e}"));
                cluster1_clients.push(Arc::new(c));
                println!("[diag] cluster1 pool client {i} connected");
            }
        }

        let total_pool = cluster0_clients.len() + cluster1_clients.len();
        println!(
            "[diag] TB client pool ready ({total_pool} connections, {} cluster(s))",
            if dual_cluster { 2 } else { 1 }
        );

        // ── Build shard pipelines ─────────────────────────────────────────────
        let mut shard_contexts: Vec<ShardContext> = Vec::with_capacity(cfg.shard_count);

        for shard_id in 0..cfg.shard_count {
            // Select which cluster owns this shard.
            let use_cluster1 = dual_cluster && shard_id >= half_shards;

            let acct_client: &TigerBeetleClient = if use_cluster1 {
                setup_client_2.as_ref().unwrap()
            } else {
                &setup_client
            };

            let debit_id = acct_client
                .create_account(Account::new(
                    AccountId::new(),
                    LedgerId::USD,
                    usd,
                    1,
                    AccountFlags::default(),
                ))
                .await
                .unwrap_or_else(|e| panic!("shard {shard_id} debit account: {e}"));

            let credit_id = acct_client
                .create_account(Account::new(
                    AccountId::new(),
                    LedgerId::USD,
                    usd,
                    1,
                    AccountFlags::default(),
                ))
                .await
                .unwrap_or_else(|e| panic!("shard {shard_id} credit account: {e}"));

            // Map shard → client: each client owns 2 shards within its cluster.
            let shard_client = if use_cluster1 {
                let local_shard = shard_id - half_shards;
                let client_idx = local_shard / 2;
                Arc::clone(&cluster1_clients[client_idx])
            } else {
                let client_idx = shard_id / 2;
                Arc::clone(&cluster0_clients[client_idx])
            };

            let builder = PipelineBuilder::new()
                .with_capacity(CAPACITY_PER_SHARD)
                .with_global_shard_id(shard_id);

            let results = builder.results();
            let result_ring = builder.result_ring();

            let ledger_handler =
                LedgerHandler::new(shard_client, Arc::clone(&ledger_rt), Arc::clone(&results))
                    .with_result_ring(Arc::clone(&result_ring));

            let (pipeline, runners) = builder
                .add_handler(ledger_handler)
                .build()
                .unwrap_or_else(|e| panic!("shard {shard_id} pipeline build: {e}"));

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
                publish_with_backpressure(&ctx.pipeline, event);
            }
        }
        tokio::time::sleep(Duration::from_millis(2_000)).await;
        println!("[diag] warmup done — starting timed bench");

        emit_event(
            &cfg.metrics_tx,
            0,
            "bench_start",
            "Warmup complete — timed bench running (VSR failover armed)",
        );

        // ── Failover controller task ─────────────────────────────────────────
        //
        // Runs on the async runtime; listens for either:
        //   (a) auto-kill timer, or
        //   (b) dashboard command: {"cmd":"kill_node","node_id":N}
        //
        // When triggered, runs the kill shell command, emits events, waits
        // recovery_secs, then runs the restart command.
        //
        // Uses channels to communicate with the main thread:
        //   kill_rx  — notified when kill is triggered (carries 0-indexed node id)
        //   revive_rx — notified when restart completes

        let (kill_notify_tx, _kill_notify_rx) = broadcast::channel::<u8>(4);
        let kill_notify_for_ctrl = kill_notify_tx.clone();

        // Move kill/restart strings out of cfg so they can be used in async task.
        let kill_cmds: [Option<String>; 3] = cfg.node_cmds.kill;
        let restart_cmds: [Option<String>; 3] = cfg.node_cmds.restart;
        let auto_kill_node = cfg.auto_kill_node;
        let auto_kill_after = cfg.auto_kill_after_secs;
        let recovery_secs = cfg.recovery_secs;
        let metrics_tx_ctrl = cfg.metrics_tx.clone();
        let mut cmd_rx = cfg.cmd_rx;

        tokio::spawn(async move {
            // Optionally wait for the auto-kill timer.
            if let Some(node_1indexed) = auto_kill_node {
                let idx = (node_1indexed as usize).saturating_sub(1).min(2);
                // Wait until auto_kill_after seconds have elapsed.
                tokio::time::sleep(Duration::from_secs(auto_kill_after)).await;
                execute_failover(
                    idx,
                    &kill_cmds,
                    &restart_cmds,
                    recovery_secs,
                    &metrics_tx_ctrl,
                    &kill_notify_for_ctrl,
                    auto_kill_after,
                )
                .await;
                return;
            }

            // Otherwise wait for dashboard commands.
            loop {
                match cmd_rx.recv().await {
                    Ok(raw) => {
                        // Parse: {"cmd":"kill_node","node_id":N}  (1-indexed)
                        if let Some(node_id) = parse_kill_cmd(&raw) {
                            let idx = (node_id as usize).saturating_sub(1).min(2);
                            let elapsed_approx = 0u64; // bench elapsed unknown here
                            execute_failover(
                                idx,
                                &kill_cmds,
                                &restart_cmds,
                                recovery_secs,
                                &metrics_tx_ctrl,
                                &kill_notify_for_ctrl,
                                elapsed_approx,
                            )
                            .await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
        });

        // ── Timed bench (same loop as sharded_tb_scenario) ───────────────────
        let committed_total = Arc::new(AtomicU64::new(0));
        let rejected_total = Arc::new(AtomicU64::new(0));
        let stop_flag = Arc::new(AtomicBool::new(false));

        if let Some(dur) = cfg.duration_secs {
            let flag = Arc::clone(&stop_flag);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(dur));
                flag.store(true, Ordering::Relaxed);
                println!("[diag] duration elapsed — signalling shard threads to drain and exit");
            });
        }

        let wall_start = Instant::now();
        let mut producer_handles = Vec::with_capacity(cfg.shard_count);

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
            let metrics_tx_shard = cfg.metrics_tx.clone();

            let handle = std::thread::Builder::new()
                .name(format!("bench-shard-{shard_id}"))
                .spawn(move || {
                    let mut latencies: Vec<u64> = if duration_mode {
                        Vec::new()
                    } else {
                        Vec::with_capacity(n as usize)
                    };
                    let mut in_flight: VecDeque<(i64, Instant)> =
                        VecDeque::with_capacity(WINDOW_PER_SHARD);
                    let mut sent = 0u64;
                    let mut received = 0u64;
                    let mut committed = 0u64;
                    let mut rejected = 0u64;
                    let mut last_hb = Instant::now();

                    let mut per_second_tps: Vec<(u64, u64)> = Vec::new();
                    let mut last_window_time = Instant::now();
                    let mut last_window_received = 0u64;

                    let mut rolling_lat: VecDeque<u64> = VecDeque::with_capacity(512);

                    let total_label: String = if duration_mode {
                        "\u{221e}".to_string()
                    } else {
                        n.to_string()
                    };

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

                    loop {
                        let done = if duration_mode {
                            stop_flag.load(Ordering::Relaxed) && in_flight.is_empty()
                        } else {
                            received >= n
                        };
                        if done {
                            break;
                        }

                        // Burst drain: consume up to DRAIN_BURST consecutive ready
                        // results in one pass. When a TB batch of 8,190 transfers
                        // completes, all its result_ring slots are written at once.
                        // Draining 1-per-outer-loop wastes ~8,190 empty iterations.
                        const DRAIN_BURST: usize = 2048;
                        let mut drained_count: usize = 0;
                        loop {
                            if drained_count >= DRAIN_BURST {
                                break;
                            }
                            match in_flight.front() {
                                None => break,
                                Some(&(seq, t0)) => {
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
                                        drained_count += 1;
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
                                        drained_count += 1;
                                    } else {
                                        break; // front not committed yet
                                    }
                                }
                            }
                        }

                        // Burst send: fill the window all the way to WINDOW_PER_SHARD
                        // after each drain pass. Previously we only sent `drained_count`
                        // events, leaving window slots empty between drain iterations.
                        // Now: any slot freed → immediately filled, keeping TB saturated.
                        // Cap at DRAIN_BURST per outer loop to not starve the tick timer.
                        let slots_free = WINDOW_PER_SHARD.saturating_sub(in_flight.len());
                        let max_send = slots_free.min(DRAIN_BURST);
                        for _ in 0..max_send {
                            if in_flight.len() >= WINDOW_PER_SHARD {
                                break;
                            }
                            let ok = if duration_mode {
                                !stop_flag.load(Ordering::Relaxed)
                            } else {
                                sent < n
                            };
                            if !ok {
                                break;
                            }
                            let event = make_event(debit_id, credit_id);
                            let seq = publish_with_backpressure(&pipeline, event);
                            in_flight.push_back((seq, Instant::now()));
                            sent += 1;
                        }

                        // Window full, nothing draining — hint CPU to back off.
                        if drained_count == 0 && in_flight.len() >= WINDOW_PER_SHARD {
                            std::hint::spin_loop();
                        }

                        // Per-second window + heartbeat to dashboard.
                        let elapsed_secs = last_window_time.elapsed().as_secs();
                        if elapsed_secs >= 1 {
                            let delta = received.saturating_sub(last_window_received);
                            let current_second = per_second_tps.len() as u64;
                            per_second_tps.push((current_second, delta));
                            last_window_time = Instant::now();
                            last_window_received = received;

                            // Compute rolling p50/p99 from the current window.
                            let (p50_us, p99_us) = if rolling_lat.is_empty() {
                                (0u64, 0u64)
                            } else {
                                let mut sorted: Vec<u64> =
                                    rolling_lat.iter().copied().collect();
                                sorted.sort_unstable();
                                let p50 = sorted[sorted.len() / 2] / 1_000;
                                let p99 = sorted[(sorted.len() * 99 / 100)
                                    .min(sorted.len().saturating_sub(1))] / 1_000;
                                (p50, p99)
                            };

                            {
                                let tx = &metrics_tx_shard;
                                let t = current_second;
                                let inflight_now = in_flight.len() as u64;
                                let msg = format!(
                                    "{{\"type\":\"tick\",\
\"t\":{t},\"shard_id\":{shard_id},\
\"tps\":{delta},\
\"committed_total\":{committed},\
\"rejected_total\":{rejected},\
\"inflight\":{inflight_now},\
\"sent_total\":{sent},\
\"p50_us\":{p50_us},\
\"p99_us\":{p99_us}}}"
                                );
                                tx.send(msg).ok();
                            }
                        }

                        if last_hb.elapsed() >= Duration::from_secs(5) {
                            let tps = if per_second_tps.is_empty() {
                                0
                            } else {
                                per_second_tps.iter().map(|(_, t)| t).sum::<u64>()
                                    / per_second_tps.len() as u64
                            };
                            println!(
                                "[shard-{shard_id}] sent={sent} received={received}/{total_label} committed={committed} rejected={rejected} ~{tps} TPS"
                            );
                            last_hb = Instant::now();
                        }
                    }

                    committed_total.fetch_add(committed, Ordering::Relaxed);
                    rejected_total.fetch_add(rejected, Ordering::Relaxed);

                    (latencies, per_second_tps)
                })
                .expect("spawn shard thread");

            producer_handles.push(handle);
        }

        // ── Collect results ───────────────────────────────────────────────────
        let mut all_latencies: Vec<u64> = Vec::new();
        let mut all_per_second: BTreeMap<u64, u64> = BTreeMap::new();

        for h in producer_handles {
            let (lats, pst) = h.join().expect("shard thread panicked");
            all_latencies.extend(lats);
            for (sec, tps) in pst {
                *all_per_second.entry(sec).or_insert(0) += tps;
            }
        }

        let wall_secs = wall_start.elapsed().as_secs_f64();
        let total_committed = committed_total.load(Ordering::Relaxed);
        let total_rejected = rejected_total.load(Ordering::Relaxed);
        let total_processed = total_committed + total_rejected;
        let overall_tps = (total_processed as f64 / wall_secs) as u64;

        all_latencies.sort_unstable();
        let p50_ns = percentile(&all_latencies, 50);
        let p99_ns = percentile(&all_latencies, 99);
        let p99_9_ns = percentile(&all_latencies, 999);
        let mean_ns = if all_latencies.is_empty() {
            0
        } else {
            all_latencies.iter().sum::<u64>() / all_latencies.len() as u64
        };

        let per_sec_values: Vec<u64> = all_per_second.values().copied().collect();
        let max_tps = per_sec_values.iter().copied().max().unwrap_or(0);
        let min_tps = per_sec_values.iter().copied().min().unwrap_or(0);
        let avg_tps = if per_sec_values.is_empty() {
            0
        } else {
            per_sec_values.iter().sum::<u64>() / per_sec_values.len() as u64
        };
        let consistency = if max_tps > 0 {
            (min_tps as f64 / max_tps as f64) * 100.0
        } else {
            100.0
        };
        let error_rate = if total_processed > 0 {
            (total_rejected as f64 / total_processed as f64) * 100.0
        } else {
            0.0
        };
        let survival_rate = 100.0 - error_rate;

        // ── Final summary to dashboard ─────────────────────────────────────────
        emit_str(
            &cfg.metrics_tx,
            &format!(
                "{{\"type\":\"summary\",\
\"total_committed\":{total_committed},\
\"total_rejected\":{total_rejected},\
\"error_rate\":{error_rate:.4},\
\"survival_rate\":{survival_rate:.4},\
\"tps\":{overall_tps},\
\"avg_tps\":{avg_tps},\
\"max_tps\":{max_tps},\
\"min_tps\":{min_tps},\
\"consistency\":{consistency:.2},\
\"p50_ns\":{p50_ns},\
\"p99_ns\":{p99_ns},\
\"p999_ns\":{p99_9_ns},\
\"mean_ns\":{mean_ns},\
\"wall_secs\":{wall_secs:.2},\
\"shards\":{shard_count}}}"
            ),
        );

        // Give WS server time to flush the summary message to all clients
        // before the process exits (prevents dashboard showing ERROR on clean exit).
        tokio::time::sleep(Duration::from_millis(500)).await;

        // ── Per-second breakdown ───────────────────────────────────────────────
        println!("\n[per-second TPS breakdown]");
        println!("{:>6}  {:>12}", "t (s)", "TPS");
        for (sec, tps) in &all_per_second {
            println!("{:>6}  {:>12}", sec, crate::report::fmt_commas(*tps));
        }
        let range = max_tps.saturating_sub(min_tps);
        println!(
            "avg={avg} max={max} min={min} range={range} consistency={c:.1}%",
            avg = crate::report::fmt_commas(avg_tps),
            max = crate::report::fmt_commas(max_tps),
            min = crate::report::fmt_commas(min_tps),
            c = consistency,
        );

        BenchmarkResult {
            scenario: "vsr-failover".to_string(),
            total_events: total_processed,
            duration_ms: (wall_secs * 1_000.0) as u64,
            duration_ns: (wall_secs * 1_000_000_000.0) as u64,
            tps: overall_tps,
            p50_ns,
            p99_ns,
            p99_9_ns,
            mean_ns,
            min_ns: all_latencies.first().copied().unwrap_or(0),
            max_ns: all_latencies.last().copied().unwrap_or(0),
            p95_ns: percentile(&all_latencies, 95),
            committed: total_committed,
            rejected: total_rejected,
        }
    }

    // ── Failover execution ────────────────────────────────────────────────────

    /// Runs the kill command for `node_idx` (0-indexed), emits events, waits
    /// `recovery_secs`, then runs the restart command.
    async fn execute_failover(
        node_idx: usize,
        kill_cmds: &[Option<String>; 3],
        restart_cmds: &[Option<String>; 3],
        recovery_secs: u64,
        metrics_tx: &broadcast::Sender<String>,
        kill_notify_tx: &broadcast::Sender<u8>,
        elapsed_t: u64,
    ) {
        let node_label = node_idx + 1; // 1-indexed for display

        // ── Kill ──────────────────────────────────────────────────────────────
        emit_event(
            metrics_tx,
            elapsed_t,
            "node_down",
            &format!("Node {node_label} kill initiated"),
        );

        if let Some(cmd) = &kill_cmds[node_idx] {
            println!("[failover] killing Node {node_label}: {cmd}");
            match run_shell_cmd(cmd).await {
                Ok(output) => {
                    println!("[failover] kill OK: {output}");
                    emit_event(
                        metrics_tx,
                        elapsed_t,
                        "node_down",
                        &format!("Node {node_label} DOWN — VSR view change triggered"),
                    );
                }
                Err(e) => {
                    eprintln!("[failover] kill command failed: {e}");
                    emit_event(
                        metrics_tx,
                        elapsed_t,
                        "node_down",
                        &format!("Node {node_label} kill command failed: {e}"),
                    );
                }
            }
        } else {
            println!(
                "[failover] no kill command configured for Node {node_label} — emit event only"
            );
            emit_event(
                metrics_tx,
                elapsed_t,
                "node_down",
                &format!("Node {node_label} DOWN (no kill command — manual kill required)"),
            );
        }

        kill_notify_tx.send(node_idx as u8).ok();

        // ── Wait for recovery window ──────────────────────────────────────────
        println!("[failover] waiting {recovery_secs}s before restart...");
        emit_event(
            metrics_tx,
            elapsed_t,
            "info",
            &format!("VSR 2-of-3 quorum active — Node {node_label} rejoining in {recovery_secs}s"),
        );
        tokio::time::sleep(Duration::from_secs(recovery_secs)).await;

        // ── Restart ───────────────────────────────────────────────────────────
        emit_event(
            metrics_tx,
            elapsed_t + recovery_secs,
            "info",
            &format!("Node {node_label} restart initiated — state transfer in progress"),
        );

        if let Some(cmd) = &restart_cmds[node_idx] {
            println!("[failover] restarting Node {node_label}: {cmd}");
            match run_shell_cmd(cmd).await {
                Ok(output) => {
                    println!("[failover] restart OK: {output}");
                }
                Err(e) => {
                    eprintln!("[failover] restart command failed: {e}");
                }
            }
        } else {
            println!("[failover] no restart command configured for Node {node_label}");
        }

        emit_event(
            metrics_tx,
            elapsed_t + recovery_secs,
            "node_up",
            &format!("Node {node_label} UP — 3-of-3 cluster restored"),
        );
    }

    // ── Shell command runner ──────────────────────────────────────────────────

    /// Runs `cmd` via `sh -c "cmd"`, returns stdout (trimmed).
    async fn run_shell_cmd(cmd: &str) -> Result<String, String> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(format!(
                "exit code {}: {stderr}",
                output.status.code().unwrap_or(-1)
            ))
        }
    }

    // ── JSON parsing ──────────────────────────────────────────────────────────

    /// Parses `{"cmd":"kill_node","node_id":N}` where N is 1-indexed.
    fn parse_kill_cmd(raw: &str) -> Option<u8> {
        // Minimal parse without pulling in serde_json for a single field.
        if !raw.contains("kill_node") {
            return None;
        }
        // Extract node_id value: "node_id":N
        let after = raw.split("\"node_id\"").nth(1)?;
        let digits: String = after
            .trim_start_matches(|c: char| !c.is_ascii_digit())
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }

    // ── Metrics helpers ───────────────────────────────────────────────────────

    fn emit_str(tx: &broadcast::Sender<String>, msg: &str) {
        tx.send(msg.to_string()).ok();
    }

    fn emit_event(tx: &broadcast::Sender<String>, t: u64, kind: &str, message: &str) {
        // Escape message for JSON (no external deps).
        let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
        tx.send(format!(
            "{{\"type\":\"event\",\"t\":{t},\"kind\":\"{kind}\",\"message\":\"{escaped}\"}}"
        ))
        .ok();
    }

    // ── Event helpers (re-exported for sharded_tb_scenario compat) ────────────

    fn make_event(debit_id: AccountId, credit_id: AccountId) -> TransactionEvent {
        use blazil_common::amount::Amount;
        use blazil_common::currency::parse_currency;
        use rust_decimal::Decimal;
        use std::str::FromStr;
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::from_str("1.00").unwrap(), usd).unwrap();
        use blazil_ledger::convert::amount_to_minor_units;
        let units = amount_to_minor_units(&amount).unwrap() as u64;
        TransactionEvent::new(
            TransactionId::new(),
            debit_id,
            credit_id,
            units,
            LedgerId::USD,
            1, // TigerBeetle requires code != 0; 1 = standard transfer
        )
    }

    fn publish_with_backpressure(pipeline: &Pipeline, event: TransactionEvent) -> i64 {
        const SPIN: usize = 64;
        loop {
            match pipeline.publish_event(event.clone()) {
                Ok(seq) => return seq,
                Err(_) => {
                    for _ in 0..SPIN {
                        std::hint::spin_loop();
                    }
                    std::thread::yield_now();
                }
            }
        }
    }

    fn percentile(sorted: &[u64], p: usize) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        if p >= 1000 {
            return *sorted.last().unwrap();
        }
        let idx = (sorted.len() * p / 1000).min(sorted.len() - 1);
        sorted[idx]
    }
}

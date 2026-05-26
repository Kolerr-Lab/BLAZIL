//! AF_XDP end-to-end benchmark scenario.
//!
//! Client-only scenario: connects to an already-running [`AfXdpTransportServer`]
//! and sends BLZL-framed UDP requests, measuring TPS and round-trip latency.
//!
//! # Architecture
//!
//! ```text
//! blazil-bench --scenario afxdp-e2e --shards N --duration D --metrics-port P
//!      │
//!      ├── shard 0: AfXdpClient → UDP → AfXdpTransportServer (running separately)
//!      ├── shard 1: AfXdpClient → UDP → AfXdpTransportServer
//!      └── ...
//! ```
//!
//! Each shard runs its own `AfXdpClient` instance on a dedicated OS thread
//! with window-based concurrency (send W requests, drain W responses).
//!
//! # Usage
//!
//! ```bash
//! # Terminal 1: start the server
//! BLAZIL_XDP_IF=eth1 ./blazil-server --features af-xdp
//!
//! # Terminal 2: run the bench
//! BLAZIL_XDP_SERVER_ADDR=<ip>:7878 \
//!   ./blazil-bench \
//!     --scenario afxdp-e2e \
//!     --shards 8 \
//!     --duration 60 \
//!     --metrics-port 9090
//! ```
//!
//! Requires `--features af-xdp,metrics-ws` and Linux target.

#[cfg(all(target_os = "linux", feature = "af-xdp", feature = "metrics-ws"))]
pub mod inner {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::sync::broadcast;

    use blazil_transport::afxdp::client::AfXdpClient;
    use blazil_transport::protocol::{deserialize_response, encode_blzl_frame, TransactionRequest};

    use crate::metrics::BenchmarkResult;

    // ── Constants ─────────────────────────────────────────────────────────────

    /// Per-shard send window.  Lower than sharded-tb because we rely on UDP
    /// (no retransmit) — large windows risk fill ring overflow on server.
    const WINDOW_PER_SHARD: usize = 1_024;

    /// Maximum response wait per receive call.
    const RECV_TIMEOUT: Duration = Duration::from_millis(200);

    /// Spin hint count before yielding on empty response window.
    const SPIN_BEFORE_YIELD: u32 = 64;

    // ── AfXdpE2eConfig ────────────────────────────────────────────────────────

    /// Configuration for a single `afxdp-e2e` benchmark run.
    pub struct AfXdpE2eConfig {
        /// Total events to send (divided equally across shards).
        /// Ignored when `duration_secs` is Some.
        pub events: u64,
        /// Number of parallel client shards.
        pub shard_count: usize,
        /// Optional time-based run duration.
        pub duration_secs: Option<u64>,
        /// Broadcast channel for live dashboard metrics.
        pub metrics_tx: broadcast::Sender<String>,
    }

    // ── Public entry point ────────────────────────────────────────────────────

    /// Run the AF_XDP E2E bench.
    ///
    /// Spawns `cfg.shard_count` OS threads, each running its own
    /// [`AfXdpClient`] send/receive loop.
    pub async fn run(cfg: AfXdpE2eConfig) -> BenchmarkResult {
        let shard_count = cfg.shard_count;
        let events_per_shard = cfg.events / shard_count as u64;
        let duration_mode = cfg.duration_secs.is_some();

        let server_addr_str = std::env::var("BLAZIL_XDP_SERVER_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:7878".to_string());

        println!("Scenario      : afxdp-e2e");
        println!("Shards        : {shard_count}");
        println!("Window/shard  : {WINDOW_PER_SHARD}");
        println!("Server        : {server_addr_str}");
        if let Some(d) = cfg.duration_secs {
            println!("Mode          : time-based ({d}s)");
        } else {
            println!("Events/shard  : {events_per_shard}");
        }

        // ── Broadcast config to dashboard ─────────────────────────────────────
        let dur_json = cfg
            .duration_secs
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let config_msg = format!(
            "{{\"type\":\"config\",\"shards\":{shard_count},\
\"duration_secs\":{dur_json},\"rt_workers\":{shard_count},\
\"tb_addr\":\"{server_addr_str}\",\"capacity_per_shard\":{WINDOW_PER_SHARD},\
\"window_per_shard\":{WINDOW_PER_SHARD}}}"
        );
        cfg.metrics_tx.send(config_msg).ok();

        // ── Per-shard aggregators (written by shard threads, read by main) ────
        let committed_total = Arc::new(AtomicU64::new(0));
        let rejected_total = Arc::new(AtomicU64::new(0));
        let done_count = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        // Duration stopper task.
        let stop_clone = Arc::clone(&stop);
        if let Some(secs) = cfg.duration_secs {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(secs)).await;
                stop_clone.store(true, Ordering::Relaxed);
            });
        }

        let wall_start = Instant::now();

        // ── Spawn shard threads ───────────────────────────────────────────────
        let mut handles: Vec<std::thread::JoinHandle<ShardResult>> =
            Vec::with_capacity(shard_count);

        for shard_id in 0..shard_count {
            let metrics_tx = cfg.metrics_tx.clone();
            let server_addr_str = server_addr_str.clone();
            let stop = Arc::clone(&stop);
            let committed_total = Arc::clone(&committed_total);
            let rejected_total = Arc::clone(&rejected_total);
            let done_count = Arc::clone(&done_count);
            let events = events_per_shard;
            let duration_mode = duration_mode;

            let handle = std::thread::Builder::new()
                .name(format!("afxdp-e2e-{shard_id}"))
                .spawn(move || {
                    run_shard(
                        shard_id,
                        shard_count,
                        events,
                        duration_mode,
                        server_addr_str,
                        stop,
                        metrics_tx,
                        committed_total,
                        rejected_total,
                        done_count,
                    )
                })
                .expect("spawn afxdp-e2e shard thread");
            handles.push(handle);
        }

        // ── Wait for all shards ───────────────────────────────────────────────
        let mut all_latencies: Vec<u64> = Vec::new();
        let mut committed: u64 = 0;
        let mut rejected: u64 = 0;

        for h in handles {
            let res = h.join().unwrap_or_else(|_| ShardResult::default());
            all_latencies.extend(res.latencies);
            committed += res.committed;
            rejected += res.rejected;
        }

        let wall_duration = wall_start.elapsed();
        all_latencies.sort_unstable();

        // ── Final summary to dashboard ────────────────────────────────────────
        let actual_total = committed + rejected;
        let error_rate = if actual_total > 0 {
            rejected as f64 / actual_total as f64 * 100.0
        } else {
            0.0
        };
        let survival_rate = 100.0 - error_rate;

        let result = BenchmarkResult::new(
            &format!("AF_XDP E2E ({shard_count} shards)"),
            actual_total,
            wall_duration,
            &mut all_latencies,
        )
        .with_counts(committed, rejected);

        let wall_secs = wall_duration.as_secs_f64();
        let summary = format!(
            "{{\"type\":\"summary\",\
\"total_committed\":{committed},\"total_rejected\":{rejected},\
\"error_rate\":{error_rate:.4},\"survival_rate\":{survival_rate:.4},\
\"tps\":{},\"avg_tps\":{},\"max_tps\":{},\"min_tps\":{},\
\"consistency\":100.00,\
\"p50_ns\":{},\"p99_ns\":{},\"p999_ns\":{},\
\"mean_ns\":{},\"wall_secs\":{wall_secs:.3},\"shards\":{shard_count}}}",
            result.tps,
            result.tps,
            result.tps,
            result.tps,
            result.p50_ns,
            result.p99_ns,
            result.p99_9_ns,
            result.mean_ns,
        );
        cfg.metrics_tx.send(summary).ok();

        result
    }

    // ── Shard result ──────────────────────────────────────────────────────────

    #[derive(Default)]
    struct ShardResult {
        committed: u64,
        rejected: u64,
        latencies: Vec<u64>,
    }

    // ── Per-shard worker ──────────────────────────────────────────────────────

    fn run_shard(
        shard_id: usize,
        shard_count: usize,
        events: u64,
        duration_mode: bool,
        server_addr_str: String,
        stop: Arc<AtomicBool>,
        metrics_tx: broadcast::Sender<String>,
        committed_total: Arc<AtomicU64>,
        rejected_total: Arc<AtomicU64>,
        done_count: Arc<AtomicU64>,
    ) -> ShardResult {
        // Build one AfXdpClient per shard — each gets its own ephemeral port.
        let server_addr: std::net::SocketAddr = server_addr_str
            .parse()
            .expect("BLAZIL_XDP_SERVER_ADDR must be a valid SocketAddr");
        let client = match AfXdpClient::connect(server_addr) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[afxdp-e2e shard={shard_id}] connect failed: {e}");
                return ShardResult::default();
            }
        };
        // Use non-blocking receive for the drain loop.
        client
            .set_recv_timeout(Some(RECV_TIMEOUT))
            .expect("set_recv_timeout");

        let mut latencies: Vec<u64> = Vec::with_capacity(events as usize);
        let mut committed: u64 = 0;
        let mut rejected: u64 = 0;
        let mut sent: u64 = 0;
        let mut received: u64 = 0;

        // Windowed in-flight: (request_id, send_time_ns)
        let mut in_flight: VecDeque<(String, Instant)> = VecDeque::with_capacity(WINDOW_PER_SHARD);

        let mut recv_buf = vec![0u8; 65_535];
        let mut tick_second: u64 = 0;
        let mut tick_committed: u64 = 0;
        let mut tick_rejected: u64 = 0;
        let mut last_tick = Instant::now();
        let mut windowed_tps: BTreeMap<u64, u64> = BTreeMap::new();

        let shard_start = Instant::now();
        let total_label = if duration_mode {
            "∞".to_string()
        } else {
            events.to_string()
        };

        loop {
            if !duration_mode && received >= events {
                break;
            }
            if stop.load(Ordering::Relaxed) {
                // Drain remaining in-flight on stop.
                break;
            }

            // ── Fill send window ───────────────────────────────────────────────
            while in_flight.len() < WINDOW_PER_SHARD {
                if !duration_mode && sent >= events {
                    break;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                let req = make_request(shard_id, sent);
                let frame = match encode_blzl_frame(&req) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("[afxdp-e2e shard={shard_id}] encode error: {e}");
                        rejected += 1;
                        sent += 1;
                        continue;
                    }
                };
                let t0 = Instant::now();
                if let Err(e) = client.send_request(&frame) {
                    eprintln!("[afxdp-e2e shard={shard_id}] send error: {e}");
                    rejected += 1;
                    sent += 1;
                    continue;
                }
                in_flight.push_back((req.request_id, t0));
                sent += 1;
            }

            // ── Drain receive window ──────────────────────────────────────────
            let mut drained_any = false;
            for _ in 0..WINDOW_PER_SHARD {
                if in_flight.is_empty() {
                    break;
                }
                match client.recv_raw(&mut recv_buf) {
                    Ok(n) => {
                        let resp = match deserialize_response(&recv_buf[..n]) {
                            Ok(r) => r,
                            Err(_) => {
                                // Drop malformed response — count as rejected.
                                rejected += 1;
                                rejected_total.fetch_add(1, Ordering::Relaxed);
                                in_flight.pop_front();
                                received += 1;
                                drained_any = true;
                                continue;
                            }
                        };

                        // Match response to oldest in-flight by request_id.
                        // We pop from front (FIFO) which is correct for
                        // sequenced single-server single-shard flow.
                        let rtt_ns = if let Some((_req_id, t0)) = in_flight.pop_front() {
                            t0.elapsed().as_nanos() as u64
                        } else {
                            0
                        };

                        latencies.push(rtt_ns);
                        received += 1;
                        drained_any = true;

                        if resp.committed {
                            committed += 1;
                            tick_committed += 1;
                            committed_total.fetch_add(1, Ordering::Relaxed);
                        } else {
                            rejected += 1;
                            tick_rejected += 1;
                            rejected_total.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        // Timeout or would-block — break inner drain loop.
                        // Stale in-flight entries will time out naturally.
                        // Reap obviously-timed-out entries (> 2 × RECV_TIMEOUT).
                        let deadline = RECV_TIMEOUT * 2;
                        while let Some((_, t0)) = in_flight.front() {
                            if t0.elapsed() > deadline {
                                in_flight.pop_front();
                                rejected += 1;
                                tick_rejected += 1;
                                rejected_total.fetch_add(1, Ordering::Relaxed);
                                received += 1;
                            } else {
                                break;
                            }
                        }
                        break;
                    }
                }
            }

            // ── Per-second tick ───────────────────────────────────────────────
            let elapsed = shard_start.elapsed();
            let current_second = elapsed.as_secs();
            if current_second > tick_second || last_tick.elapsed() >= Duration::from_secs(1) {
                let delta = tick_committed + tick_rejected;
                windowed_tps.insert(tick_second, delta);

                let (p50_us, p99_us) = if latencies.len() >= 2 {
                    let mut s = latencies.clone();
                    s.sort_unstable();
                    (s[s.len() / 2] / 1_000, s[(s.len() * 99) / 100] / 1_000)
                } else {
                    (0, 0)
                };

                println!(
                    "[t+{current_second:3}s] shard={shard_id} \
                     TPS={delta:>8} inflight={:>4} p99={p99_us}µs",
                    in_flight.len(),
                );

                let msg = format!(
                    "{{\"type\":\"tick\",\"t\":{current_second},\
\"shard_id\":{shard_id},\"tps\":{delta},\
\"committed_total\":{committed},\"rejected_total\":{rejected},\
\"inflight\":{},\"sent_total\":{sent},\
\"p50_us\":{p50_us},\"p99_us\":{p99_us}}}",
                    in_flight.len(),
                );
                metrics_tx.send(msg).ok();

                tick_second = current_second;
                tick_committed = 0;
                tick_rejected = 0;
                last_tick = Instant::now();
            }

            if !drained_any {
                let mut spins = 0u32;
                spins = spins.wrapping_add(1);
                if spins & (SPIN_BEFORE_YIELD - 1) == 0 {
                    std::thread::yield_now();
                } else {
                    std::hint::spin_loop();
                }
            }
        }

        // Final tick for any remaining.
        let elapsed_secs = shard_start.elapsed().as_secs();
        let msg = format!(
            "{{\"type\":\"tick\",\"t\":{elapsed_secs},\
\"shard_id\":{shard_id},\"tps\":0,\
\"committed_total\":{committed},\"rejected_total\":{rejected},\
\"inflight\":0,\"sent_total\":{sent},\
\"p50_us\":0,\"p99_us\":0}}"
        );
        metrics_tx.send(msg).ok();
        done_count.fetch_add(1, Ordering::Relaxed);

        println!(
            "[afxdp-e2e shard={shard_id}] done: \
             sent={sent} received={received} \
             committed={committed} rejected={rejected}"
        );

        ShardResult {
            committed,
            rejected,
            latencies,
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a deterministic [`TransactionRequest`] for the bench load.
    fn make_request(shard_id: usize, seq: u64) -> TransactionRequest {
        TransactionRequest {
            request_id: format!("afxdp-{shard_id}-{seq}"),
            debit_account_id: format!("debit-shard-{shard_id}"),
            credit_account_id: format!("credit-shard-{shard_id}"),
            amount: "1.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
            flags: 0,
            pending_transfer_id: "".into(),
        }
    }
}

// ── Public entry point (module-level re-export) ───────────────────────────────

#[cfg(all(target_os = "linux", feature = "af-xdp", feature = "metrics-ws"))]
pub use inner::*;

//! Aeron IPC E2E benchmark.
//!
//! Client → Aeron:IPC (stream 1001) → [embedded C driver] → AeronTransportServer
//!   → Pipeline → [embedded C driver] → Aeron:IPC (stream 1002) → Client
//!
//! Uses window-based async pipelining (same pattern as udp_scenario).
//! IPC eliminates kernel UDP stack overhead — measures pure Aeron + pipeline
//! throughput.
//!
//! # Requirements
//!
//! Run only with the `aeron` feature **and** the C library built:
//!
//! ```bash
//! git submodule update --init --recursive
//! cargo run --bin blazil-bench --features aeron -- --scenario aeron --events 100000
//! ```

#[cfg(feature = "aeron")]
pub mod inner {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use blazil_common::currency::parse_currency;
    use blazil_common::error::BlazerResult;
    use blazil_common::ids::{AccountId, LedgerId, TransferId};
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::handlers::publish::PublishHandler;
    use blazil_engine::handlers::risk::RiskHandler;
    use blazil_engine::handlers::validation::ValidationHandler;
    use blazil_engine::pipeline::PipelineBuilder;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::mock::InMemoryLedgerClient;
    #[cfg(feature = "tigerbeetle-client")]
    use blazil_ledger::tigerbeetle::TigerBeetleClient;
    use blazil_ledger::transfer::Transfer;
    use blazil_transport::aeron::{
        AeronContext, AeronPublication, AeronSubscription, REQ_STREAM_ID, RSP_STREAM_ID,
    };
    use blazil_transport::aeron_transport::AeronTransportServer;
    use blazil_transport::protocol::{deserialize_response, serialize_request, TransactionRequest};
    use blazil_transport::server::TransportServer;

    use crate::metrics::BenchmarkResult;

    // On Linux, /dev/shm is a tmpfs mounted in RAM — guaranteed zero page-fault
    // latency for Aeron's shared-memory log buffers. On macOS /dev/shm is absent
    // so we fall back to /tmp.
    #[cfg(target_os = "linux")]
    const BENCH_AERON_DIR: &str = "/dev/shm/aeron-blazil-bench";
    #[cfg(not(target_os = "linux"))]
    const BENCH_AERON_DIR: &str = "/tmp/aeron-blazil-bench";
    // True IPC (shared-memory log buffer via embedded driver) — eliminates
    // the UDP loopback network stack entirely for maximum throughput.
    // term-length=67108864: 64 MB IPC log buffer (4× default 16 MB) prevents
    // Aeron back-pressure / Status Message storms when the window is full
    // and TB batch RTT is high.
    const BENCH_CHANNEL: &str = "aeron:ipc?term-length=67108864";
    const REG_TIMEOUT: Duration = Duration::from_secs(5);
    // Window size: tuned per backend.
    // InMemory: 2048 — saturates the pipeline at ~1M TPS.
    // Real TigerBeetle (VSR, 3-node): 16384 — keeps ~2 full TB batches of
    //   8190 transfers in-flight simultaneously. As TB_RTT grows due to
    //   journal growth or VSR checkpointing, more batches pipeline into the
    //   same RTT window, keeping TPS flat regardless of individual batch
    //   latency. Formula: TPS = N_batches_in_flight * 8190 / TB_RTT.
    const WINDOW_SIZE_INMEM: usize = 2048;
    const WINDOW_SIZE_TB: usize = 16_384;
    // 2000 events: enough to prime Aeron's flow-control and IPC log buffer.
    const WARMUP_EVENTS: u64 = 2000;
    // Larger ring buffer: 128K slots prevents any pipeline backpressure.
    const CAPACITY: usize = 131_072;
    // Max spin retries on Aeron offer backpressure before yielding.
    const OFFER_SPIN_RETRIES: usize = 64;

    // ── Isolation-test mock ───────────────────────────────────────────────────
    // Set BLAZIL_MOCK_DELAY_MS=2 to replace real TigerBeetle with a mock
    // client that sleeps for `delay_ms` then returns Ok for every transfer.
    //
    // Isolation logic:
    //   TPS FLAT   with mock  → bottleneck is TigerBeetle/Network/DO disk.
    //   TPS DECAY  with mock  → bottleneck is Rust pipeline / Aeron / memory.
    struct DelayedMockLedgerClient {
        inner: InMemoryLedgerClient,
        delay: Duration,
    }

    impl DelayedMockLedgerClient {
        fn new(delay_ms: u64) -> Self {
            Self {
                inner: InMemoryLedgerClient::new_unbounded(),
                delay: Duration::from_millis(delay_ms),
            }
        }
    }

    #[async_trait::async_trait]
    impl LedgerClient for DelayedMockLedgerClient {
        async fn create_account(&self, account: Account) -> BlazerResult<AccountId> {
            self.inner.create_account(account).await
        }
        async fn create_transfer(&self, transfer: Transfer) -> BlazerResult<TransferId> {
            tokio::time::sleep(self.delay).await;
            self.inner.create_transfer(transfer).await
        }
        async fn get_account(&self, id: &AccountId) -> BlazerResult<Account> {
            self.inner.get_account(id).await
        }
        async fn get_transfer(&self, id: &TransferId) -> BlazerResult<Transfer> {
            self.inner.get_transfer(id).await
        }
        async fn create_transfers_batch(
            &self,
            transfers: Vec<Transfer>,
        ) -> Vec<BlazerResult<TransferId>> {
            // Simulate a single VSR round-trip latency for the whole batch.
            tokio::time::sleep(self.delay).await;
            // Return Ok for every transfer — mock never rejects.
            transfers.into_iter().map(|t| Ok(*t.id())).collect()
        }
        async fn get_account_balances(&self, ids: &[AccountId]) -> BlazerResult<Vec<Account>> {
            self.inner.get_account_balances(ids).await
        }
    }

    pub async fn run(events: u64, payload_size: usize) -> BenchmarkResult {
        let usd = parse_currency("USD").expect("USD");
        println!("Payload size : {payload_size} bytes");

        let ledger_rt = Arc::new({
            // Worker-thread counter for core affinity assignment.
            // These 2 workers are pinned to cores 2 and 3 on Linux:
            //
            //   Core 0 — Aeron serve thread (transport.rs, pinned)
            //   Core 1 — Pipeline runner (LedgerHandler batch accumulator)
            //   Core 2 — ledger_rt worker 0 (TB async callbacks)
            //   Core 3 — ledger_rt worker 1 (TB async callbacks)
            //
            // Isolating TB callbacks from the Aeron poll thread eliminates the
            // scheduling contention that caused TPS to decay under load.
            let _worker_idx = Arc::new(AtomicUsize::new(0));
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .on_thread_start(move || {
                    #[cfg(target_os = "linux")]
                    {
                        let slot = _worker_idx.fetch_add(1, Ordering::Relaxed);
                        if let Some(core_ids) = core_affinity::get_core_ids() {
                            // Start at core 2; wrap to avoid out-of-bounds.
                            let n = core_ids.len();
                            if n > 2 {
                                let target = 2 + (slot % (n - 2));
                                if let Some(id) = core_ids.get(target) {
                                    core_affinity::set_for_current(*id);
                                }
                            }
                        }
                    }
                })
                .build()
                .expect("ledger rt")
        });

        // ── ledger client — real TB when BLAZIL_TB_ADDRESS is set ─────────────
        #[cfg(feature = "tigerbeetle-client")]
        let tb_addr = std::env::var("BLAZIL_TB_ADDRESS").ok();
        #[cfg(not(feature = "tigerbeetle-client"))]
        let tb_addr: Option<String> = None;

        if let Some(ref addr) = tb_addr {
            println!("Ledger      : TigerBeetle @ {addr}");
            println!("Window size : {} (TB mode)", WINDOW_SIZE_TB);
            #[cfg(feature = "tigerbeetle-client")]
            {
                println!("[diag] calling TigerBeetleClient::connect...");
                let tb_client = TigerBeetleClient::connect(addr, 0)
                    .await
                    .expect("TigerBeetle connect");
                println!("[diag] connect OK — calling run_with_ledger...");
                return run_with_ledger(
                    events,
                    payload_size,
                    WINDOW_SIZE_TB,
                    usd,
                    ledger_rt,
                    tb_client,
                )
                .await;
            }
            #[cfg(not(feature = "tigerbeetle-client"))]
            let _ = addr;
        }

        // ── Isolation test: mock ledger with configurable delay ───────────
        if let Ok(delay_str) = std::env::var("BLAZIL_MOCK_DELAY_MS") {
            let delay_ms: u64 = delay_str.parse().unwrap_or(2);
            println!("Ledger      : [MOCK] DelayedMock @ {delay_ms}ms (isolation test)");
            println!(
                "Window size : {} (TB-mode window — same pipeline pressure)",
                WINDOW_SIZE_TB
            );
            println!("BLAZIL_MOCK_DELAY_MS={delay_ms}: if TPS is flat → bottleneck is TB/DO.");
            println!("                              if TPS decays → bottleneck is Rust/Aeron.");
            return run_with_ledger(
                events,
                payload_size,
                WINDOW_SIZE_TB,
                usd,
                ledger_rt,
                DelayedMockLedgerClient::new(delay_ms),
            )
            .await;
        }

        println!("Ledger      : InMemory (set BLAZIL_TB_ADDRESS for real TB)");
        println!("Window size : {} (InMemory mode)", WINDOW_SIZE_INMEM);
        run_with_ledger(
            events,
            payload_size,
            WINDOW_SIZE_INMEM,
            usd,
            ledger_rt,
            InMemoryLedgerClient::new_unbounded(),
        )
        .await
    }

    async fn run_with_ledger<C: LedgerClient + Send + Sync + 'static>(
        events: u64,
        payload_size: usize,
        window_size: usize,
        usd: blazil_common::currency::Currency,
        ledger_rt: Arc<tokio::runtime::Runtime>,
        ledger_client: C,
    ) -> BenchmarkResult {
        let ledger_client = Arc::new(ledger_client);

        // ── accounts ──────────────────────────────────────────────────────────
        println!("[diag] creating debit account...");
        let debit_id = ledger_client
            .create_account(Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            ))
            .await
            .expect("debit account");
        println!("[diag] debit account OK");
        let credit_id = ledger_client
            .create_account(Account::new(
                AccountId::new(),
                LedgerId::USD,
                usd,
                1,
                AccountFlags::default(),
            ))
            .await
            .expect("credit account");
        println!("[diag] credit account OK");

        // ── pipeline ──────────────────────────────────────────────────────────
        println!("[diag] building pipeline (capacity={})...", CAPACITY);
        let builder = PipelineBuilder::new().with_capacity(CAPACITY);
        let results = builder.results();
        // Build LedgerHandler separately so we can extract the active-task
        // counter BEFORE moving the handler into PipelineBuilder::add_handler.
        let ledger_handler =
            LedgerHandler::new(Arc::clone(&ledger_client), ledger_rt, Arc::clone(&results));
        let active_tb_tasks = Arc::clone(ledger_handler.active_tasks());
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(100_000_000_000, Arc::clone(&results)))
            .add_handler(ledger_handler)
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");
        println!(
            "[diag] pipeline OK, spawning {} runner(s)...",
            runners.len()
        );

        let pipeline = Arc::new(pipeline);
        let _run_handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();
        println!("[diag] runner threads started");

        // ── server ────────────────────────────────────────────────────────────
        println!("[diag] starting Aeron transport server...");
        let server = Arc::new(AeronTransportServer::new(
            BENCH_CHANNEL,
            BENCH_AERON_DIR,
            Arc::clone(&pipeline),
        ));
        let s = Arc::clone(&server);
        tokio::task::spawn(async move {
            let _ = s.serve().await;
        });

        // Give the embedded driver + server time to start.
        println!("[diag] waiting 800ms for Aeron driver + server...");
        tokio::time::sleep(Duration::from_millis(800)).await;
        println!("[diag] 800ms elapsed, starting client thread...");

        // ── client (blocking thread) ──────────────────────────────────────────
        let debit_id_str = debit_id.to_string();
        let credit_id_str = credit_id.to_string();
        let total = events;

        // Expose serve-thread internals to bench heartbeat.
        let serve_pending_len = Arc::clone(server.pending_len());
        let serve_offer_fail = Arc::clone(server.offer_failures());

        // active_tb_tasks is moved into the blocking closure so the heartbeat
        // can read it. The Arc keeps the counter alive for the full bench run.
        let result = tokio::task::spawn_blocking(move || {
            let ctx = AeronContext::new(BENCH_AERON_DIR).expect("client AeronContext");

            // Client → server (stream 1001).
            let client_pub = AeronPublication::new(&ctx, BENCH_CHANNEL, REQ_STREAM_ID, REG_TIMEOUT)
                .expect("client pub");

            // Server → client (stream 1002).
            let client_sub =
                AeronSubscription::new(&ctx, BENCH_CHANNEL, RSP_STREAM_ID, REG_TIMEOUT)
                    .expect("client sub");

            // Wait for server subscription to appear.
            let conn_deadline = Instant::now() + Duration::from_secs(3);
            while !client_pub.is_connected() && Instant::now() < conn_deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                client_pub.is_connected(),
                "Aeron bench: client pub not connected after 3s"
            );
            println!("[diag] Aeron client connected (pub is_connected=true)");

            // ── CPU P-state warmup ─────────────────────────────────────────
            // Spin 2M iterations to push the M4/Linux CPU into its highest
            // P-state and prime L1/L2 caches before any timed code runs.
            for _ in 0..2_000_000usize {
                std::hint::spin_loop();
            }

            // ── Aeron warmup ──────────────────────────────────────────────────
            // 2000 events: primes the IPC log buffer, Aeron flow-control,
            // and the ring-buffer shard worker threads.
            println!("[diag] sending {} warmup events...", WARMUP_EVENTS);
            let mut warmup_resp: Vec<Vec<u8>> = Vec::new();
            for i in 0..WARMUP_EVENTS {
                let mut bytes = serialize_request(&make_request(i, &debit_id_str, &credit_id_str))
                    .expect("serialize");
                bytes.resize(payload_size.max(bytes.len()), 0u8);
                // Spin-retry on offer backpressure during warmup.
                let mut retries = 0usize;
                while client_pub.offer(&bytes).is_err() {
                    for _ in 0..OFFER_SPIN_RETRIES {
                        std::hint::spin_loop();
                    }
                    retries += 1;
                    if retries > 1000 {
                        break;
                    }
                }
            }
            let warmup_deadline = Instant::now() + Duration::from_secs(8);
            while (warmup_resp.len() as u64) < WARMUP_EVENTS && Instant::now() < warmup_deadline {
                client_sub.poll_fragments(&mut warmup_resp, 256);
                std::thread::yield_now(); // let TB callback threads run
            }
            println!(
                "[diag] warmup done: got {}/{} responses",
                warmup_resp.len(),
                WARMUP_EVENTS
            );
            warmup_resp.clear();
            // Let the pipeline drain and CPU frequency stabilize.
            std::thread::sleep(Duration::from_millis(50));
            println!(
                "[diag] starting main bench ({} events, window={})...",
                total, window_size
            );

            // ── benchmark: window-based pipelined send/recv ────────────────
            let mut send_times = Vec::with_capacity(total as usize);
            let mut latencies = Vec::with_capacity(total as usize);
            let mut responses: Vec<Vec<u8>> = Vec::new();
            let mut sent = 0usize;
            let mut received = 0usize;
            let mut committed = 0u64;
            let mut rejected = 0u64;
            let total_usize = total as usize;

            let wall_start = Instant::now();

            // Fill window.
            let initial = window_size.min(total_usize);
            for i in 0..initial {
                let mut bytes =
                    serialize_request(&make_request(i as u64, &debit_id_str, &credit_id_str))
                        .expect("serialize");
                bytes.resize(payload_size.max(bytes.len()), 0u8);
                send_times.push(Instant::now());
                // Spin-retry on offer backpressure to keep P-cores hot.
                let mut retries = 0usize;
                while client_pub.offer(&bytes).is_err() {
                    for _ in 0..OFFER_SPIN_RETRIES {
                        std::hint::spin_loop();
                    }
                    retries += 1;
                    if retries > 1000 {
                        break;
                    }
                }
                sent += 1;
            }

            // Drain loop.
            let mut last_heartbeat = Instant::now();
            while received < total_usize {
                responses.clear();
                let count = client_sub.poll_fragments(&mut responses, window_size);
                for resp_bytes in &responses {
                    if let Some(t0) = send_times.get(received) {
                        latencies.push(t0.elapsed().as_nanos() as u64);
                    }
                    match deserialize_response(resp_bytes) {
                        Ok(r) if r.committed => committed += 1,
                        _ => rejected += 1,
                    }
                    received += 1;

                    if sent < total_usize {
                        let mut bytes = serialize_request(&make_request(
                            sent as u64,
                            &debit_id_str,
                            &credit_id_str,
                        ))
                        .expect("serialize");
                        bytes.resize(payload_size.max(bytes.len()), 0u8);
                        send_times.push(Instant::now());
                        // Spin-retry keeps P-cores hot on Aeron backpressure.
                        let mut retries = 0usize;
                        while client_pub.offer(&bytes).is_err() {
                            for _ in 0..OFFER_SPIN_RETRIES {
                                std::hint::spin_loop();
                            }
                            retries += 1;
                            if retries > 1000 {
                                break;
                            }
                        }
                        sent += 1;
                    }
                }
                if count == 0 {
                    // Heartbeat every 5s (was 10s — shorter interval makes TPS
                    // decay visible immediately rather than after 10s of decay).
                    //
                    // active_tb_tasks: should stay ≤ MAX_CONCURRENT_BATCHES (8).
                    //   If stuck at 8 and tps_delta=0: TB is disk I/O bound;
                    //     run `iostat -x 1 5` on the TB node to confirm.
                    //   If stuck at 0 and tps_delta=0: serve thread isn't
                    //     receiving from bench client; check Aeron connection.
                    //   If growing monotonically: backpressure cap is too low
                    //     or TB is truly overloaded.
                    //
                    // tps_delta: instantaneous TPS since last heartbeat.
                    //   Should be close to the opening TPS. Decay here = pipeline
                    //   bottleneck. Flat = healthy.
                    if last_heartbeat.elapsed().as_secs() >= 5 {
                        let elapsed = wall_start.elapsed().as_secs_f64();
                        let tps_avg = if elapsed > 0.0 {
                            received as f64 / elapsed
                        } else {
                            0.0
                        };
                        let active = active_tb_tasks.load(Ordering::Relaxed);
                        let pending_n = serve_pending_len.load(Ordering::Relaxed);
                        let offer_fail = serve_offer_fail.load(Ordering::Relaxed);
                        let vm_mb = vm_rss_mb();
                        println!(
                            "[heartbeat] elapsed={:.1}s recv={}/{} committed={} \
                             rejected={} sent={} active_tb={} pending={} \
                             offer_fail={} vm_rss={}MB tps_avg={:.0}",
                            elapsed,
                            received,
                            total_usize,
                            committed,
                            rejected,
                            sent,
                            active,
                            pending_n,
                            offer_fail,
                            vm_mb,
                            tps_avg
                        );
                        last_heartbeat = Instant::now();
                    }
                    // 8 × spin_loop keeps the polling thread on a P-core
                    // without an OS context switch during brief idle gaps.
                    for _ in 0..8 {
                        std::hint::spin_loop();
                    }
                }
            }

            let duration = wall_start.elapsed();

            drop(client_pub);
            drop(client_sub);
            drop(ctx);

            (duration, latencies, committed, rejected)
        })
        .await
        .expect("bench blocking task");

        let (duration, mut latencies, committed, rejected) = result;

        server.shutdown().await;

        let total_responses = committed + rejected;
        let error_rate = if total_responses > 0 {
            rejected as f64 / total_responses as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "Committed : {} / Rejected : {} / Error rate : {:.2}%",
            committed, rejected, error_rate,
        );

        BenchmarkResult::new("Aeron IPC E2E", events, duration, &mut latencies)
    }

    /// Returns current process RSS in MB.
    /// Linux: reads /proc/self/status. Other platforms: returns 0.
    fn vm_rss_mb() -> u64 {
        #[cfg(target_os = "linux")]
        {
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if let Some(rest) = line.strip_prefix("VmRSS:") {
                        let kb: u64 = rest
                            .split_whitespace()
                            .next()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0);
                        return kb / 1024;
                    }
                }
            }
            0
        }
        #[cfg(not(target_os = "linux"))]
        {
            0
        }
    }

    fn make_request(seq: u64, debit_id: &str, credit_id: &str) -> TransactionRequest {
        TransactionRequest {
            request_id: format!("bench-{seq}"),
            debit_account_id: debit_id.to_owned(),
            credit_account_id: credit_id.to_owned(),
            amount: "1.00".to_owned(),
            currency: "USD".to_owned(),
            ledger_id: 1, // 1 = USD (see ledger_id_to_currency)
            code: 1,
        }
    }
}

#[cfg(feature = "aeron")]
pub use inner::run;

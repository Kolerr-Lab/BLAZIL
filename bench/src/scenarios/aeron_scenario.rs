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
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId};
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::handlers::publish::PublishHandler;
    use blazil_engine::handlers::risk::RiskHandler;
    use blazil_engine::handlers::validation::ValidationHandler;
    use blazil_engine::pipeline::PipelineBuilder;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::mock::InMemoryLedgerClient;
    use blazil_transport::aeron::{
        AeronContext, AeronPublication, AeronSubscription, REQ_STREAM_ID, RSP_STREAM_ID,
    };
    use blazil_transport::aeron_transport::AeronTransportServer;
    use blazil_transport::protocol::{deserialize_response, serialize_request, TransactionRequest};
    use blazil_transport::server::TransportServer;

    use crate::metrics::BenchmarkResult;

    const BENCH_AERON_DIR: &str = "/tmp/aeron-blazil-bench";
    // True IPC (shared-memory log buffer via embedded driver) — eliminates
    // the UDP loopback network stack entirely for maximum throughput.
    const BENCH_CHANNEL: &str = "aeron:ipc";
    const REG_TIMEOUT: Duration = Duration::from_secs(5);
    // Larger window keeps the pipeline fully saturated at target ~1M TPS:
    // 1M TPS × 1ms P99 latency ≈ 1000 in-flight; 2048 gives 2× headroom.
    const WINDOW_SIZE: usize = 2048;
    // 2000 events: enough to prime Aeron's flow-control and IPC log buffer.
    const WARMUP_EVENTS: u64 = 2000;
    // Larger ring buffer: 128K slots prevents any pipeline backpressure.
    const CAPACITY: usize = 131_072;
    // Max spin retries on Aeron offer backpressure before yielding.
    const OFFER_SPIN_RETRIES: usize = 64;

    pub async fn run(events: u64, payload_size: usize) -> BenchmarkResult {
        let usd = parse_currency("USD").expect("USD");
        println!("Payload size : {payload_size} bytes");

        // ── pipeline ──────────────────────────────────────────────────────────
        let ledger_client = Arc::new(InMemoryLedgerClient::new_unbounded());
        let ledger_rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("ledger rt"),
        );

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

        let builder = PipelineBuilder::new().with_capacity(CAPACITY);
        let results = builder.results();
        let (pipeline, runners) = builder
            .add_handler(ValidationHandler::new(Arc::clone(&results)))
            .add_handler(RiskHandler::new(100_000_000_000, Arc::clone(&results)))
            .add_handler(LedgerHandler::new(
                ledger_client,
                ledger_rt,
                Arc::clone(&results),
            ))
            .add_handler(PublishHandler::new(Arc::clone(&results)))
            .build()
            .expect("pipeline");

        let pipeline = Arc::new(pipeline);
        let _run_handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

        // ── server ────────────────────────────────────────────────────────────
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
        tokio::time::sleep(Duration::from_millis(800)).await;

        // ── client (blocking thread) ──────────────────────────────────────────
        let debit_id_str = debit_id.to_string();
        let credit_id_str = credit_id.to_string();
        let total = events;

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

            // ── CPU P-state warmup ─────────────────────────────────────────
            // Spin 2M iterations to push the M4/Linux CPU into its highest
            // P-state and prime L1/L2 caches before any timed code runs.
            for _ in 0..2_000_000usize {
                std::hint::spin_loop();
            }

            // ── Aeron warmup ──────────────────────────────────────────────────
            // 2000 events: primes the IPC log buffer, Aeron flow-control,
            // and the ring-buffer shard worker threads.
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
            }
            warmup_resp.clear();
            // Let the pipeline drain and CPU frequency stabilize.
            std::thread::sleep(Duration::from_millis(50));

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
            let initial = WINDOW_SIZE.min(total_usize);
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
            while received < total_usize {
                responses.clear();
                let count = client_sub.poll_fragments(&mut responses, WINDOW_SIZE);
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

    fn make_request(seq: u64, debit_id: &str, credit_id: &str) -> TransactionRequest {
        TransactionRequest {
            request_id: format!("bench-{seq}"),
            debit_account_id: debit_id.to_owned(),
            credit_account_id: credit_id.to_owned(),
            amount: "1.00".to_owned(),
            currency: "USD".to_owned(),
            ledger_id: 0,
            code: 1,
        }
    }
}

#[cfg(feature = "aeron")]
pub use inner::run;

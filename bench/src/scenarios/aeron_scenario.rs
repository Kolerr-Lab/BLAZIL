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
        AeronContext, AeronPublication, AeronSubscription, EmbeddedAeronDriver,
        REQ_STREAM_ID, RSP_STREAM_ID,
    };
    use blazil_transport::aeron_transport::AeronTransportServer;
    use blazil_transport::protocol::{
        deserialize_response, serialize_request, TransactionRequest,
    };
    use blazil_transport::server::TransportServer;

    use crate::metrics::BenchmarkResult;

    const BENCH_AERON_DIR: &str = "/tmp/aeron-blazil-bench";
    const BENCH_CHANNEL: &str = "aeron:udp?endpoint=127.0.0.1:41235";
    const REG_TIMEOUT: Duration = Duration::from_secs(5);
    const WINDOW_SIZE: usize = 512;
    const WARMUP_EVENTS: u64 = 500;
    const CAPACITY: usize = 65_536;

    pub async fn run(events: u64) -> BenchmarkResult {
        let usd = parse_currency("USD").expect("USD");

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
        tokio::time::sleep(Duration::from_millis(600)).await;

        // ── client (blocking thread) ──────────────────────────────────────────
        let debit_id_str = debit_id.to_string();
        let credit_id_str = credit_id.to_string();
        let total = events;

        let result = tokio::task::spawn_blocking(move || {
            let ctx =
                AeronContext::new(BENCH_AERON_DIR).expect("client AeronContext");

            // Client → server (stream 1001).
            let client_pub =
                AeronPublication::new(&ctx, BENCH_CHANNEL, REQ_STREAM_ID, REG_TIMEOUT)
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

            // ── warmup ────────────────────────────────────────────────────────
            let mut warmup_resp: Vec<Vec<u8>> = Vec::new();
            for i in 0..WARMUP_EVENTS {
                let bytes =
                    serialize_request(&make_request(i, &debit_id_str, &credit_id_str))
                        .expect("serialize");
                client_pub.offer(&bytes).ok();
            }
            let warmup_deadline = Instant::now() + Duration::from_secs(5);
            while (warmup_resp.len() as u64) < WARMUP_EVENTS
                && Instant::now() < warmup_deadline
            {
                client_sub.poll_fragments(&mut warmup_resp, 100);
            }
            warmup_resp.clear();
            std::thread::sleep(Duration::from_millis(20));

            // ── benchmark: window-based pipelined send/recv ────────────────
            let mut send_times = Vec::with_capacity(total as usize);
            let mut latencies = Vec::with_capacity(total as usize);
            let mut responses: Vec<Vec<u8>> = Vec::new();
            let mut sent = 0usize;
            let mut received = 0usize;
            let total_usize = total as usize;

            let wall_start = Instant::now();

            // Fill window.
            let initial = WINDOW_SIZE.min(total_usize);
            for i in 0..initial {
                let bytes =
                    serialize_request(&make_request(i as u64, &debit_id_str, &credit_id_str))
                        .expect("serialize");
                send_times.push(Instant::now());
                client_pub.offer(&bytes).expect("offer");
                sent += 1;
            }

            // Drain loop.
            while received < total_usize {
                responses.clear();
                let count = client_sub.poll_fragments(&mut responses, WINDOW_SIZE);
                for _ in 0..responses.len() {
                    if let Some(t0) = send_times.get(received) {
                        latencies.push(t0.elapsed().as_nanos() as u64);
                    }
                    received += 1;

                    if sent < total_usize {
                        let bytes = serialize_request(&make_request(
                            sent as u64,
                            &debit_id_str,
                            &credit_id_str,
                        ))
                        .expect("serialize");
                        send_times.push(Instant::now());
                        client_pub.offer(&bytes).expect("offer");
                        sent += 1;
                    }
                }
                if count == 0 {
                    std::hint::spin_loop();
                }
            }

            let duration = wall_start.elapsed();

            drop(client_pub);
            drop(client_sub);
            drop(ctx);

            (duration, latencies)
        })
        .await
        .expect("bench blocking task");

        let (duration, mut latencies) = result;

        server.shutdown().await;

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

//! Q3 — End-to-end latency over real TCP.
//!
//! Client → TCP → TransportServer → Engine → InMemoryLedgerClient.
//!
//! Uses ONE persistent TCP connection for all transactions (warmup + benchmark).
//! This matches production client behaviour and avoids macOS TIME_WAIT
//! port exhaustion that occurs when reconnecting per transaction.
//!
//! Warmup:    100 events  (send_batch, single connection)
//! Benchmark: 10K events  (single persistent stream, per-event timing)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blazil_common::amount::Amount;
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
use blazil_transport::mock::MockTransportClient;
use blazil_transport::protocol::{
    deserialize_response, serialize_request, Frame, TransactionRequest,
};
use blazil_transport::server::TransportServer;
use blazil_transport::tcp::TcpTransportServer;
use rust_decimal::Decimal;
use tokio::net::TcpStream;

use crate::metrics::BenchmarkResult;

const WARMUP_EVENTS: u64 = 100;
const CAPACITY: usize    = 65_536;

/// Run the TCP scenario 3 times and return the median-TPS result.
pub async fn run(events: u64) -> BenchmarkResult {
    let mut results = Vec::with_capacity(3);
    for _ in 0..3 {
        results.push(run_once(events).await);
    }
    results.sort_unstable_by_key(|r: &BenchmarkResult| r.tps);
    results.remove(1)
}

async fn run_once(events: u64) -> BenchmarkResult {
    let usd = parse_currency("USD").expect("USD");

    // ── shared ledger + runtime ───────────────────────────────────────────────
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());
    let ledger_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("ledger runtime"),
    );

    // Pre-create accounts directly — the pipeline handles transfers only.
    let debit_id  = client.create_account(Account::new(
        AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default(),
    )).await.expect("debit account");
    let credit_id = client.create_account(Account::new(
        AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default(),
    )).await.expect("credit account");

    let max_amount = Amount::new(Decimal::new(100_000_000_000, 2), parse_currency("USD").expect("USD"))
        .expect("max amount");

    // ── pipeline ─────────────────────────────────────────────────────────────
    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(CAPACITY)
        .add_handler(ValidationHandler)
        .add_handler(RiskHandler::new(max_amount))
        .add_handler(LedgerHandler::new(client.clone(), ledger_rt.clone()))
        .add_handler(PublishHandler::new())
        .build()
        .expect("pipeline build");

    let ring_buffer = Arc::clone(pipeline.ring_buffer());
    let pipeline    = Arc::new(pipeline);
    let run_handle  = runner.run();

    // ── server ───────────────────────────────────────────────────────────────
    let server = Arc::new(TcpTransportServer::new(
        "127.0.0.1:0",
        Arc::clone(&pipeline),
        ring_buffer,
        1_000,
    ));
    let s = Arc::clone(&server);
    tokio::spawn(async move { let _ = s.serve().await; });

    // Wait for the listener to bind and update bound_addr.
    let addr = loop {
        tokio::time::sleep(Duration::from_millis(2)).await;
        let a = server.local_addr_async().await;
        if a != "127.0.0.1:0" {
            break a;
        }
    };

    // ── warmup: send_batch reuses ONE TCP connection ──────────────────────────
    let warmup_client = MockTransportClient::new(&addr);
    let warmup_reqs: Vec<TransactionRequest> = (0..WARMUP_EVENTS)
        .map(|_| make_request(&debit_id, &credit_id))
        .collect();
    let _ = warmup_client.send_batch(warmup_reqs).await;

    // Brief pause so the server drains warmup before measurement starts.
    tokio::time::sleep(Duration::from_millis(5)).await;

    // ── benchmark: ONE persistent TcpStream, per-event timing ────────────────
    //
    // Open a single connection (same pattern as send_batch) and loop, measuring
    // each request individually. Zero new ports opened — no TIME_WAIT buildup.
    let mut stream = TcpStream::connect(&addr)
        .await
        .expect("benchmark connect");

    let mut latencies = Vec::with_capacity(events as usize);
    let wall_start = Instant::now();

    for _ in 0..events {
        let req     = make_request(&debit_id, &credit_id);
        let payload = serialize_request(&req).expect("serialize");

        let t0 = Instant::now();
        Frame::write_frame(&mut stream, &payload)
            .await
            .expect("write_frame");
        let frame = Frame::read_frame(&mut stream)
            .await
            .expect("read_frame");
        latencies.push(t0.elapsed().as_nanos() as u64);

        // Consume the frame to keep the stream in sync.
        let _ = deserialize_response(&frame.payload);
    }

    let duration = wall_start.elapsed();

    // Drop the stream so the server sees EOF and closes its connection task.
    drop(stream);

    // ── teardown ─────────────────────────────────────────────────────────────
    server.shutdown().await;
    pipeline.stop();

    let result = BenchmarkResult::new("End-to-End TCP", events, duration, &mut latencies);

    // Runtime::drop() blocks — must happen outside an async context.
    tokio::task::block_in_place(move || {
        run_handle.join().expect("runner panicked");
        drop(Arc::try_unwrap(ledger_rt).ok());
    });

    result
}

// ── helpers ──────────────────────────────────────────────────────────────────

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn make_request(debit_id: &AccountId, credit_id: &AccountId) -> TransactionRequest {
    let id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Format as a hyphenated UUID string so the transport layer accepts it without warnings.
    // Using a fixed prefix + counter in the final 12 hex nibbles.
    let request_id = format!("10000000-0000-4000-8000-{:012x}", id & 0x0000_ffff_ffff_ffff);
    TransactionRequest {
        request_id,
        debit_account_id:  debit_id.to_string(),
        credit_account_id: credit_id.to_string(),
        amount:            "1.00".to_owned(),
        currency:          "USD".to_owned(),
        ledger_id:         1,
        code:              1,
    }
}

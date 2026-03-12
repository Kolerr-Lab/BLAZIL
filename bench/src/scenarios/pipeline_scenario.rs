//! Q2 — Full pipeline throughput with `InMemoryLedgerClient`.
//!
//! All four handlers active: ValidationHandler → RiskHandler →
//! LedgerHandler → PublishHandler.  No network, no disk I/O.
//! Uses `InMemoryLedgerClient::new_unbounded()` to skip balance checks
//! so a single account pair can absorb millions of debits.

use std::sync::Arc;
use std::time::Instant;

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::event::TransactionEvent;
use blazil_engine::handlers::ledger::LedgerHandler;
use blazil_engine::handlers::publish::PublishHandler;
use blazil_engine::handlers::risk::RiskHandler;
use blazil_engine::handlers::validation::ValidationHandler;
use blazil_engine::pipeline::PipelineBuilder;
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::mock::InMemoryLedgerClient;
use rust_decimal::Decimal;

use crate::metrics::BenchmarkResult;
use crate::scenarios::ring_buffer_scenario::{publish_with_backpressure, wait_for_drain};

const WARMUP_EVENTS: u64 = 10_000;
const CAPACITY: usize = 1_048_576;

/// Run the pipeline scenario 3 times and return the median-TPS result.
///
/// Uses `spawn_blocking` so the benchmark runs on a dedicated OS thread where
/// creating and dropping a `tokio::Runtime` is allowed.
pub async fn run(events: u64) -> BenchmarkResult {
    let mut results: Vec<BenchmarkResult> = Vec::with_capacity(3);
    for _ in 0..3 {
        let r = tokio::task::spawn_blocking(move || run_once_blocking(events))
            .await
            .expect("benchmark thread panicked");
        results.push(r);
    }
    results.sort_unstable_by_key(|r| r.tps);
    results.remove(1)
}

/// Synchronous benchmark body — runs on a `spawn_blocking` thread so
/// creating, using, and dropping a `tokio::Runtime` is permitted.
fn run_once_blocking(events: u64) -> BenchmarkResult {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("ledger runtime"),
    );

    let usd = parse_currency("USD").expect("USD");
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());

    let debit_id = rt
        .block_on(client.create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        )))
        .expect("debit account");
    let credit_id = rt
        .block_on(client.create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        )))
        .expect("credit account");

    let amount = Amount::new(Decimal::new(1_00, 2), usd).expect("amount");
    let max_amount = Amount::new(
        Decimal::new(100_000_000_000, 2),
        parse_currency("USD").expect("USD"),
    )
    .expect("max amount");

    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(CAPACITY)
        .add_handler(ValidationHandler)
        .add_handler(RiskHandler::new(max_amount))
        .add_handler(LedgerHandler::new(client.clone(), rt.clone()))
        .add_handler(PublishHandler::new())
        .build()
        .expect("pipeline build");

    let rb = Arc::clone(pipeline.ring_buffer());
    let handle = runner.run();

    let template = TransactionEvent::new(
        TransactionId::new(),
        debit_id,
        credit_id,
        amount,
        LedgerId::USD,
        1,
    );

    // ── warmup ───────────────────────────────────────────────────────────────
    let mut last_seq: i64 = -1;
    for _ in 0..WARMUP_EVENTS {
        last_seq = publish_with_backpressure(&pipeline, template.clone());
    }
    wait_for_drain(&rb, last_seq);

    // ── benchmark ────────────────────────────────────────────────────────────
    let mut latencies = Vec::with_capacity(events as usize);
    let start = Instant::now();

    for _ in 0..events {
        let t0 = Instant::now();
        last_seq = publish_with_backpressure(&pipeline, template.clone());
        latencies.push(t0.elapsed().as_nanos() as u64);
    }

    let duration = start.elapsed();
    wait_for_drain(&rb, last_seq);

    pipeline.stop();
    handle.join().expect("runner panicked");
    // rt is dropped here on the blocking thread — safe.

    BenchmarkResult::new("Pipeline (in-memory)", events, duration, &mut latencies)
}

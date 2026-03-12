//! Q4 — Full pipeline with real TigerBeetle (env-gated).
//!
//! Only runs when `BLAZIL_TB_ADDRESS` is set in the environment.
//! Requires the `tigerbeetle-client` feature to compile the real client.

use crate::metrics::BenchmarkResult;

/// Run the TigerBeetle scenario if `BLAZIL_TB_ADDRESS` is set.
///
/// Returns `None` and prints a skip message when the env var is absent
/// or the feature is not compiled in.
#[cfg(feature = "tigerbeetle-client")]
pub async fn run(events: u64) -> Option<BenchmarkResult> {
    use std::sync::Arc;
    use std::time::Instant;

    use crate::scenarios::ring_buffer_scenario::{publish_with_backpressure, wait_for_drain};
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::TransactionId;
    use blazil_common::ids::{AccountId, LedgerId};
    use blazil_engine::event::TransactionEvent;
    use blazil_engine::handlers::ledger::LedgerHandler;
    use blazil_engine::handlers::publish::PublishHandler;
    use blazil_engine::handlers::risk::RiskHandler;
    use blazil_engine::handlers::validation::ValidationHandler;
    use blazil_engine::pipeline::PipelineBuilder;
    use blazil_ledger::account::{Account, AccountFlags};
    use blazil_ledger::client::LedgerClient;
    use blazil_ledger::tigerbeetle::TigerBeetleClient;
    use rust_decimal::Decimal;

    let addr = match std::env::var("BLAZIL_TB_ADDRESS") {
        Ok(a) => a,
        Err(_) => {
            println!("  TigerBeetle: SKIPPED (BLAZIL_TB_ADDRESS not set)");
            return None;
        }
    };

    println!("  TigerBeetle: connecting to {addr}...");

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );

    let tb = match rt.block_on(TigerBeetleClient::connect(&addr, 0)) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            println!("  TigerBeetle: SKIPPED (connect failed: {e})");
            return None;
        }
    };

    let usd = parse_currency("USD").expect("USD");

    // Create two accounts in TigerBeetle.
    let debit_id = rt
        .block_on(tb.create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd.clone(),
            1,
            AccountFlags::default(),
        )))
        .expect("debit account");
    let credit_id = rt
        .block_on(tb.create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd.clone(),
            1,
            AccountFlags::default(),
        )))
        .expect("credit account");

    let amount = Amount::new(Decimal::new(1_00, 2), usd).expect("amount");
    let max_amount = Amount::new(
        Decimal::new(1_000_000_000_00, 2),
        parse_currency("USD").expect("USD"),
    )
    .expect("max amount");

    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(65_536)
        .add_handler(ValidationHandler)
        .add_handler(RiskHandler::new(max_amount))
        .add_handler(LedgerHandler::new(tb.clone(), rt.clone()))
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

    // Warmup: 100 events (TigerBeetle is slower — keep warmup small).
    let mut last_seq: i64 = -1;
    for _ in 0..100u64 {
        last_seq = publish_with_backpressure(&pipeline, template.clone());
    }
    wait_for_drain(&rb, last_seq);

    // Benchmark.
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

    Some(BenchmarkResult::new(
        "TigerBeetle (real)",
        events,
        duration,
        &mut latencies,
    ))
}

#[cfg(not(feature = "tigerbeetle-client"))]
pub async fn run(_events: u64) -> Option<BenchmarkResult> {
    println!(
        "  TigerBeetle: SKIPPED \
         (compile with --features tigerbeetle-client)"
    );
    None
}

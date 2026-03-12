use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};

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
use blazil_common::error::BlazerError;
use rust_decimal::Decimal;

fn bench_single_tx_latency(c: &mut Criterion) {
    let usd = parse_currency("USD").expect("USD");

    let client = Arc::new(InMemoryLedgerClient::new_unbounded());
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime"),
    );

    let debit_id = rt.block_on(client.create_account(Account::new(
        AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default(),
    ))).expect("debit account");
    let credit_id = rt.block_on(client.create_account(Account::new(
        AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default(),
    ))).expect("credit account");

    let amount = Amount::new(Decimal::new(1_00, 2), usd).expect("amount");
    let max_amount = Amount::new(Decimal::new(100_000_000_000, 2), usd).expect("max");

    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(65_536)
        .add_handler(ValidationHandler)
        .add_handler(RiskHandler::new(max_amount))
        .add_handler(LedgerHandler::new(client.clone(), rt.clone()))
        .add_handler(PublishHandler::new())
        .build()
        .expect("pipeline");

    let rb = Arc::clone(pipeline.ring_buffer());
    let _handle = runner.run();

    let template = TransactionEvent::new(
        TransactionId::new(), debit_id, credit_id, amount, LedgerId::USD, 1,
    );

    c.bench_function("single_tx_latency", |b| {
        b.iter(|| {
            // Publish with backpressure retry.
            let seq = loop {
                match pipeline.publish_event(template.clone()) {
                    Ok(s) => break s,
                    Err(BlazerError::RingBufferFull { .. }) => std::hint::spin_loop(),
                    Err(e) => panic!("bench error: {e}"),
                }
            };
            // Wait for this specific event to be processed (result written).
            loop {
                let result = unsafe { &*rb.get(seq) }.result.clone();
                if result.is_some() {
                    break;
                }
                std::hint::spin_loop();
            }
        });
    });

    pipeline.stop();
}

criterion_group!(benches, bench_single_tx_latency);
criterion_main!(benches);

//! End-to-end smoke tests: Engine Pipeline → TigerBeetleClient → TigerBeetle server.
//!
//! These tests verify the full stack:
//! `TransactionEvent` → `PipelineBuilder` → `ValidationHandler`
//! → `LedgerHandler(TigerBeetleClient)` → TigerBeetle server
//!
//! # Running
//!
//! **macOS users:** TigerBeetle requires `io_uring` (Linux-only).
//! Run these tests in CI (GitHub Actions), a Linux VM, or WSL2.
//!
//! 1. Start TigerBeetle (Linux only):
//!    ```sh
//!    docker run --rm -v docker_tigerbeetle-data:/data \
//!      ghcr.io/tigerbeetle/tigerbeetle:latest \
//!      format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle
//!    docker compose -f infra/docker/docker-compose.dev.yml up tigerbeetle -d
//!    ```
//!
//! 2. Run tests:
//!    ```sh
//!    BLAZIL_TB_ADDRESS=127.0.0.1:3000 \
//!      cargo test -p blazil-transport \
//!      --test e2e_tigerbeetle -- --nocapture
//!    ```
//!
//! If `BLAZIL_TB_ADDRESS` is not set, all tests are skipped.

use std::env;
use std::sync::Arc;
use std::time::Instant;

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::event::TransactionEvent;
use blazil_engine::handlers::ledger::LedgerHandler;
use blazil_engine::handlers::validation::ValidationHandler;
use blazil_engine::pipeline::PipelineBuilder;
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::tigerbeetle::TigerBeetleClient;
use rust_decimal::Decimal;

// ── Environment gating ────────────────────────────────────────────────────────

/// Returns `None` if `BLAZIL_TB_ADDRESS` is not set, so callers can skip.
fn tb_address() -> Option<String> {
    env::var("BLAZIL_TB_ADDRESS").ok()
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Connects to TigerBeetle using the provided address.
async fn connect_tb(address: &str) -> TigerBeetleClient {
    TigerBeetleClient::connect(address, 0)
        .await
        .expect("failed to connect to TigerBeetle")
}

fn usd_amount(value: &str) -> Amount {
    let usd = parse_currency("USD").unwrap();
    Amount::new(Decimal::from_str_exact(value).unwrap(), usd).unwrap()
}

/// Pre-creates two TigerBeetle accounts and returns (debit_id, credit_id).
async fn create_accounts_for_test(
    client: &TigerBeetleClient,
    debit_constrained: bool,
) -> (AccountId, AccountId) {
    let usd = parse_currency("USD").unwrap();

    let debit_flags = if debit_constrained {
        AccountFlags {
            debits_must_not_exceed_credits: true,
            ..AccountFlags::default()
        }
    } else {
        AccountFlags::default()
    };

    let debit_acc = Account::new(AccountId::new(), LedgerId::USD, usd, 1, debit_flags);
    let credit_acc = Account::new(
        AccountId::new(),
        LedgerId::USD,
        usd,
        1,
        AccountFlags::default(),
    );

    let t0 = Instant::now();
    let debit_id = client
        .create_account(debit_acc)
        .await
        .expect("create debit account failed");
    let credit_id = client
        .create_account(credit_acc)
        .await
        .expect("create credit account failed");
    tracing::info!(
        elapsed_ms = t0.elapsed().as_millis(),
        "test accounts created"
    );

    (debit_id, credit_id)
}

// ── End-to-end tests ──────────────────────────────────────────────────────────

/// Verifies that a valid 250.00 USD transfer flows from the pipeline through
/// `LedgerHandler(TigerBeetleClient)` and is committed to TigerBeetle.
///
/// Demonstrates elapsed-time logging for every TigerBeetle operation.
#[test]
fn e2e_successful_transaction() {
    let Some(address) = tb_address() else {
        eprintln!("SKIP: BLAZIL_TB_ADDRESS not set — e2e test skipped");
        return;
    };

    // Build a Tokio async runtime (LedgerHandler uses block_on internally)
    let rt = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Tokio runtime failed"),
    );

    // Connect and pre-create accounts outside the pipeline
    let tb1 = rt.block_on(connect_tb(&address));
    let (debit_id, credit_id) = rt.block_on(create_accounts_for_test(&tb1, false));

    // Build the pipeline: ValidationHandler → LedgerHandler(TigerBeetleClient)
    let tb2 = Arc::new(rt.block_on(connect_tb(&address)));
    let tb3 = Arc::clone(&tb2); // kept for post-test verification

    let ledger_handler = LedgerHandler::new(Arc::clone(&tb2), Arc::clone(&rt));

    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(1024)
        .add_handler(ValidationHandler)
        .add_handler(ledger_handler)
        .build()
        .expect("pipeline build failed");

    let handle = runner.run();

    // Publish a valid 250.00 USD transfer event
    let t0 = Instant::now();
    let event = TransactionEvent::new(
        TransactionId::new(),
        debit_id,
        credit_id,
        usd_amount("250.00"),
        LedgerId::USD,
        1,
    );
    pipeline.publish_event(event).expect("publish_event failed");
    tracing::info!(
        elapsed_ms = t0.elapsed().as_millis(),
        "event published to pipeline"
    );

    // Allow the runner to process the event
    std::thread::sleep(std::time::Duration::from_millis(500));

    pipeline.stop();
    handle.join().expect("runner thread panicked");

    // Verify TigerBeetle recorded the balances correctly
    let t1 = Instant::now();
    let (debit_after, credit_after) = rt.block_on(async {
        let d = tb3
            .get_account(&debit_id)
            .await
            .expect("get debit account failed");
        let c = tb3
            .get_account(&credit_id)
            .await
            .expect("get credit account failed");
        (d, c)
    });
    tracing::info!(
        elapsed_ms = t1.elapsed().as_millis(),
        "balance verification completed"
    );

    assert_eq!(
        debit_after.debits_posted().value(),
        Decimal::from_str_exact("250.00").unwrap(),
        "debit account should have 250.00 USD debited"
    );
    assert_eq!(
        credit_after.credits_posted().value(),
        Decimal::from_str_exact("250.00").unwrap(),
        "credit account should have 250.00 USD credited"
    );
}

/// Verifies that a zero-amount transfer is rejected by `ValidationHandler`
/// before reaching TigerBeetle.
///
/// The mock ledger is used to confirm that `LedgerHandler` is never invoked
/// for a rejected event.
#[test]
fn e2e_rejected_transaction() {
    let Some(_) = tb_address() else {
        eprintln!("SKIP: BLAZIL_TB_ADDRESS not set — e2e test skipped");
        return;
    };

    let rt = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Tokio runtime failed"),
    );

    // Use an in-memory ledger to capture what actually reaches the LedgerHandler
    use blazil_ledger::mock::InMemoryLedgerClient;
    let mock = Arc::new(InMemoryLedgerClient::new());
    let mock_handler = LedgerHandler::new(Arc::clone(&mock), Arc::clone(&rt));

    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(1024)
        .add_handler(ValidationHandler)
        .add_handler(mock_handler)
        .build()
        .expect("pipeline build failed");

    let handle = runner.run();

    // Publish a zero-amount event — ValidationHandler must reject before LedgerHandler
    let usd = parse_currency("USD").unwrap();
    let zero_amount = Amount::zero(usd);
    let event = TransactionEvent::new(
        TransactionId::new(),
        AccountId::new(),
        AccountId::new(),
        zero_amount,
        LedgerId::USD,
        1,
    );

    pipeline.publish_event(event).expect("publish_event failed");

    // Give the runner time to process
    std::thread::sleep(std::time::Duration::from_millis(200));

    pipeline.stop();
    handle.join().expect("runner thread panicked");

    // Assert: zero transfers were committed (ValidationHandler rejected the event)
    assert_eq!(
        rt.block_on(mock.transfer_count()),
        0,
        "zero-amount transfer should be rejected before reaching the ledger"
    );
}

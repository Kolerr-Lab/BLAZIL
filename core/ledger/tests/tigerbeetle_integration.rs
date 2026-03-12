//! Integration tests for the real TigerBeetle client.
//!
//! These tests connect to a live TigerBeetle server and exercise the full
//! stack: Blazil → tigerbeetle-unofficial → TigerBeetle server → disk.
//!
//! # Running
//!
//! **macOS users:** TigerBeetle requires `io_uring`, which is Linux-only.
//! Run these tests in CI (GitHub Actions), a Linux VM, or WSL2.
//! On macOS, tests will exit with `io_uring is not available` errors.
//!
//! 1. Start TigerBeetle (Linux only):
//!    ```sh
//!    docker compose -f infra/docker/docker-compose.dev.yml up tigerbeetle -d
//!    ```
//!
//!    If the container exits with `PermissionDenied`, format the data file first:
//!    ```sh
//!    docker run --rm -v docker_tigerbeetle-data:/data \
//!      ghcr.io/tigerbeetle/tigerbeetle:latest \
//!      format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle
//!    ```
//!    Then start again:
//!    ```sh
//!    docker compose -f infra/docker/docker-compose.dev.yml up tigerbeetle -d
//!    ```
//!
//! 2. Run tests:
//!    ```sh
//!    BLAZIL_TB_ADDRESS=127.0.0.1:3000 \
//!      cargo test -p blazil-ledger --features tigerbeetle-client \
//!      --test tigerbeetle_integration -- --nocapture
//!    ```
//!
//! If `BLAZIL_TB_ADDRESS` is not set, all tests are skipped.

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::error::BlazerError;
use blazil_common::ids::{AccountId, LedgerId, TransferId};
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::tigerbeetle::TigerBeetleClient;
use blazil_ledger::transfer::Transfer;
use rust_decimal::Decimal;
use std::env;

// ── Environment-gating macro ──────────────────────────────────────────────────

/// Skips the test if `BLAZIL_TB_ADDRESS` is not set.
///
/// All TigerBeetle integration tests must be gated behind this check.
macro_rules! require_tb {
    () => {
        if env::var("BLAZIL_TB_ADDRESS").is_err() {
            eprintln!("SKIP: BLAZIL_TB_ADDRESS not set — TigerBeetle integration test skipped");
            return;
        }
    };
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Connects to TigerBeetle using the `BLAZIL_TB_ADDRESS` environment variable.
async fn connect_tb() -> TigerBeetleClient {
    let address =
        env::var("BLAZIL_TB_ADDRESS").expect("BLAZIL_TB_ADDRESS must be set (e.g. 127.0.0.1:3000)");
    TigerBeetleClient::connect(&address, 0)
        .await
        .expect("failed to connect to TigerBeetle")
}

/// Creates a USD account with a unique ID and returns it.
fn usd_account() -> Account {
    let usd = parse_currency("USD").unwrap();
    Account::new(
        AccountId::new(),
        LedgerId::USD,
        usd,
        1,
        AccountFlags::default(),
    )
}

/// Creates a USD account with `debits_must_not_exceed_credits` constraint.
fn usd_account_constrained() -> Account {
    let usd = parse_currency("USD").unwrap();
    let flags = AccountFlags {
        debits_must_not_exceed_credits: true,
        ..AccountFlags::default()
    };
    Account::new(AccountId::new(), LedgerId::USD, usd, 1, flags)
}

/// Creates a EUR account with a unique ID.
fn eur_account() -> Account {
    let eur = parse_currency("EUR").unwrap();
    Account::new(
        AccountId::new(),
        LedgerId::USD,
        eur,
        1,
        AccountFlags::default(),
    )
}

/// Creates a USD amount from a decimal value (e.g. 100.50 USD).
fn usd_amount(value: &str) -> Amount {
    let usd = parse_currency("USD").unwrap();
    Amount::new(Decimal::from_str_exact(value).unwrap(), usd).unwrap()
}

/// Creates a EUR amount from a decimal value.
fn eur_amount(value: &str) -> Amount {
    let eur = parse_currency("EUR").unwrap();
    Amount::new(Decimal::from_str_exact(value).unwrap(), eur).unwrap()
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[tokio::test]
async fn create_account_real() {
    require_tb!();
    let client = connect_tb().await;

    let account = usd_account();
    let account_id = *account.id();

    client
        .create_account(account)
        .await
        .expect("create_account should succeed");

    // Verify the account was created by fetching it back
    let fetched = client
        .get_account(&account_id)
        .await
        .expect("get_account should succeed");

    assert_eq!(fetched.id(), &account_id);
    assert_eq!(fetched.currency().code(), "USD");
    assert_eq!(fetched.debits_posted().value(), Decimal::ZERO);
    assert_eq!(fetched.credits_posted().value(), Decimal::ZERO);
}

#[tokio::test]
async fn create_transfer_real() {
    require_tb!();
    let client = connect_tb().await;

    // Create two accounts: debit and credit
    let debit_account = usd_account();
    let credit_account = usd_account();

    let debit_id = client
        .create_account(debit_account)
        .await
        .expect("debit account creation failed");
    let credit_id = client
        .create_account(credit_account)
        .await
        .expect("credit account creation failed");

    // Create a transfer of 250.00 USD
    let transfer = Transfer::new(
        TransferId::new(),
        debit_id,
        credit_id,
        usd_amount("250.00"),
        LedgerId::USD,
        1,
    )
    .unwrap();
    let transfer_id = *transfer.id();

    client
        .create_transfer(transfer)
        .await
        .expect("create_transfer should succeed");

    // Verify the transfer was recorded by fetching it back
    let fetched = client
        .get_transfer(&transfer_id)
        .await
        .expect("get_transfer should succeed");

    assert_eq!(fetched.id(), &transfer_id);
    assert_eq!(fetched.debit_account_id(), &debit_id);
    assert_eq!(fetched.credit_account_id(), &credit_id);
    assert_eq!(
        fetched.amount().value(),
        Decimal::from_str_exact("250.00").unwrap()
    );

    // Verify account balances were updated
    let debit_account_after = client
        .get_account(&debit_id)
        .await
        .expect("get_account (debit) should succeed");
    let credit_account_after = client
        .get_account(&credit_id)
        .await
        .expect("get_account (credit) should succeed");

    assert_eq!(
        debit_account_after.debits_posted().value(),
        Decimal::from_str_exact("250.00").unwrap()
    );
    assert_eq!(
        credit_account_after.credits_posted().value(),
        Decimal::from_str_exact("250.00").unwrap()
    );
}

#[tokio::test]
async fn idempotency_real() {
    require_tb!();
    let client = connect_tb().await;

    let account = usd_account();
    let _account_id = *account.id();

    // First creation succeeds
    client
        .create_account(account.clone())
        .await
        .expect("first create_account should succeed");

    // Second creation with the same ID fails with a ledger error
    // TigerBeetle returns `exists` error for duplicate IDs
    let result = client.create_account(account).await;
    assert!(result.is_err(), "duplicate create_account should fail");
    match result.unwrap_err() {
        BlazerError::Ledger(msg) => {
            assert!(
                msg.contains("create_accounts failed") || msg.contains("exists"),
                "error message should mention failure or exists, got: {}",
                msg
            );
        }
        other => panic!("expected BlazerError::Ledger, got: {:?}", other),
    }
}

#[tokio::test]
async fn insufficient_funds_real() {
    require_tb!();
    let client = connect_tb().await;

    // Create a constrained account with zero balance
    let account = usd_account_constrained();
    let account_id = client
        .create_account(account)
        .await
        .expect("create_account should succeed");

    // Create a credit-only account to receive funds
    let credit_account = usd_account();
    let credit_id = client
        .create_account(credit_account)
        .await
        .expect("credit account creation failed");

    // Attempt a transfer from the constrained account with zero balance
    // TigerBeetle should reject this with `exceeds_credits` error
    let transfer = Transfer::new(
        TransferId::new(),
        account_id,
        credit_id,
        usd_amount("100.00"),
        LedgerId::USD,
        1,
    )
    .unwrap();

    let result = client.create_transfer(transfer).await;
    assert!(
        result.is_err(),
        "transfer with insufficient funds should fail"
    );
    match result.unwrap_err() {
        BlazerError::Ledger(msg) => {
            assert!(
                msg.contains("create_transfers failed") || msg.contains("exceeds_credits"),
                "error message should mention exceeds_credits, got: {}",
                msg
            );
        }
        other => panic!("expected BlazerError::Ledger, got: {:?}", other),
    }
}

#[tokio::test]
async fn currency_round_trip_real() {
    require_tb!();
    let client = connect_tb().await;

    // Create accounts with different currencies and verify round-trip
    let usd_acc = usd_account();
    let usd_id = *usd_acc.id();

    let eur_acc = eur_account();
    let eur_id = *eur_acc.id();

    client
        .create_account(usd_acc)
        .await
        .expect("USD account creation failed");
    client
        .create_account(eur_acc)
        .await
        .expect("EUR account creation failed");

    // Fetch back and verify currency is correctly preserved via user_data_32
    let fetched_usd = client
        .get_account(&usd_id)
        .await
        .expect("get_account (USD) failed");
    let fetched_eur = client
        .get_account(&eur_id)
        .await
        .expect("get_account (EUR) failed");

    assert_eq!(fetched_usd.currency().code(), "USD");
    assert_eq!(fetched_eur.currency().code(), "EUR");

    // Create transfers and verify currency is preserved
    // We can't transfer between mismatched currencies, so we transfer within each
    let usd_credit = usd_account();
    let usd_credit_id = client
        .create_account(usd_credit)
        .await
        .expect("USD credit account creation failed");

    let eur_credit = eur_account();
    let eur_credit_id = client
        .create_account(eur_credit)
        .await
        .expect("EUR credit account creation failed");

    let usd_transfer = Transfer::new(
        TransferId::new(),
        usd_id,
        usd_credit_id,
        usd_amount("50.00"),
        LedgerId::USD,
        1,
    )
    .unwrap();
    let usd_transfer_id = *usd_transfer.id();

    let eur_transfer = Transfer::new(
        TransferId::new(),
        eur_id,
        eur_credit_id,
        eur_amount("75.50"),
        LedgerId::USD,
        1,
    )
    .unwrap();
    let eur_transfer_id = *eur_transfer.id();

    client
        .create_transfer(usd_transfer)
        .await
        .expect("USD transfer failed");
    client
        .create_transfer(eur_transfer)
        .await
        .expect("EUR transfer failed");

    // Fetch back transfers and verify currency via user_data_32
    let fetched_usd_transfer = client
        .get_transfer(&usd_transfer_id)
        .await
        .expect("get_transfer (USD) failed");
    let fetched_eur_transfer = client
        .get_transfer(&eur_transfer_id)
        .await
        .expect("get_transfer (EUR) failed");

    assert_eq!(fetched_usd_transfer.amount().currency().code(), "USD");
    assert_eq!(
        fetched_usd_transfer.amount().value(),
        Decimal::from_str_exact("50.00").unwrap()
    );

    assert_eq!(fetched_eur_transfer.amount().currency().code(), "EUR");
    assert_eq!(
        fetched_eur_transfer.amount().value(),
        Decimal::from_str_exact("75.50").unwrap()
    );
}

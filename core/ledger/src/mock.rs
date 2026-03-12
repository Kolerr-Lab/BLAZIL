//! In-memory [`LedgerClient`] implementation for tests.
//!
//! [`InMemoryLedgerClient`] stores accounts and transfers in a pair of
//! `Arc<RwLock<HashMap>>`. It fully validates double-entry rules and updates
//! account balances on every transfer, making it a faithful emulation of the
//! real ledger for unit and integration tests.
//!
//! **This is the only implementation used in tests.** No live TigerBeetle
//! server is required.
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::mock::InMemoryLedgerClient;
//! use blazil_ledger::client::LedgerClient;
//! use blazil_ledger::account::{Account, AccountFlags};
//! use blazil_ledger::transfer::Transfer;
//! use blazil_common::ids::{AccountId, LedgerId, TransferId};
//! use blazil_common::amount::Amount;
//! use blazil_common::currency::parse_currency;
//! use rust_decimal::Decimal;
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = InMemoryLedgerClient::new();
//!     let usd = parse_currency("USD").unwrap();
//!
//!     let debit_account = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
//!     let credit_account = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
//!
//!     // Seed debit account with funds first
//!     let debit_id = client.create_account(debit_account).await.unwrap();
//!     let credit_id = client.create_account(credit_account).await.unwrap();
//!
//!     let amount = Amount::new(Decimal::new(50_00, 2), usd).unwrap();
//!     let transfer = Transfer::new(TransferId::new(), debit_id, credit_id, amount, LedgerId::USD, 1).unwrap();
//!     // Note: debit account has no debits_must_not_exceed_credits here,
//!     // so overdraft is permitted.
//!     client.create_transfer(transfer).await.unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::instrument;

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, TransferId};

use crate::account::Account;
use crate::client::LedgerClient;
use crate::double_entry;
use crate::transfer::Transfer;

// ── InMemoryLedgerClient ──────────────────────────────────────────────────────

/// An in-process ledger for tests and development.
///
/// Thread-safe (`Arc + RwLock`), supporting concurrent reads and exclusive
/// writes. Cloning the client shares the underlying state.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::mock::InMemoryLedgerClient;
///
/// let client = InMemoryLedgerClient::new();
/// assert_eq!(client.blocking_account_count(), 0);
/// ```
#[derive(Clone, Debug)]
pub struct InMemoryLedgerClient {
    accounts: Arc<RwLock<HashMap<AccountId, Account>>>,
    transfers: Arc<RwLock<HashMap<TransferId, Transfer>>>,
    /// When `true`, `create_transfer` skips all validation and balance updates.
    /// **Benchmark-only behaviour** — do not use in production or test code.
    unbounded: bool,
}

impl InMemoryLedgerClient {
    /// Creates a new, empty in-memory ledger.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::mock::InMemoryLedgerClient;
    /// let client = InMemoryLedgerClient::new();
    /// ```
    pub fn new() -> Self {
        Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            transfers: Arc::new(RwLock::new(HashMap::new())),
            unbounded: false,
        }
}

    /// Creates an in-memory ledger that skips all balance validation.
    ///
    /// **Benchmark-only** — `create_transfer` returns `Ok` immediately without
    /// checking account existence, currency, or balance constraints. This allows
    /// a single account pair to absorb millions of debits during throughput tests.
    pub fn new_unbounded() -> Self {
        Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            transfers: Arc::new(RwLock::new(HashMap::new())),
            unbounded: true,
        }
    }
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::mock::InMemoryLedgerClient;
    /// let client = InMemoryLedgerClient::new();
    /// assert_eq!(client.blocking_account_count(), 0);
    /// ```
    pub async fn account_count(&self) -> usize {
        self.accounts.read().await.len()
    }

    /// Returns the number of transfers currently stored (async).
    ///
    /// Intended for test assertions after operations complete.
    pub async fn transfer_count(&self) -> usize {
        self.transfers.read().await.len()
    }

    /// Synchronous helper for use in doc-test contexts where `await` is
    /// unavailable. Do not call from async code.
    pub fn blocking_account_count(&self) -> usize {
        self.accounts.blocking_read().len()
    }
}

impl Default for InMemoryLedgerClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LedgerClient for InMemoryLedgerClient {
    /// Creates an account, rejecting duplicates.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::Duplicate`] if an account with this ID already exists.
    #[instrument(skip(self, account), fields(account_id = %account.id()))]
    async fn create_account(&self, account: Account) -> BlazerResult<AccountId> {
        let id = *account.id();
        let mut accounts = self.accounts.write().await;
        if accounts.contains_key(&id) {
            return Err(BlazerError::Duplicate {
                resource: "Account".to_owned(),
                id: id.to_string(),
            });
        }
        tracing::debug!(account_id = %id, "creating account");
        accounts.insert(id, account);
        tracing::info!(account_id = %id, "account created");
        Ok(id)
    }

    /// Creates a transfer, validates double-entry rules, and updates balances.
    ///
    /// Holds the accounts write-lock for the entire operation to ensure
    /// atomicity: no other transfer can interfere between the validation and
    /// the balance updates.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::NotFound`] if either account does not exist.
    /// - [`BlazerError::CurrencyMismatch`] if currencies disagree.
    /// - [`BlazerError::InsufficientFunds`] if the debit account is constrained
    ///   and has insufficient funds.
    #[instrument(skip(self, transfer), fields(transfer_id = %transfer.id()))]
    async fn create_transfer(&self, transfer: Transfer) -> BlazerResult<TransferId> {
        let transfer_id = *transfer.id();
        let debit_id = *transfer.debit_account_id();
        let credit_id = *transfer.credit_account_id();

        // Benchmark fast-path: skip all validation and balance updates.
        if self.unbounded {
            return Ok(transfer_id);
        }

        tracing::debug!(transfer_id = %transfer_id, debit = %debit_id, credit = %credit_id, "creating transfer");

        let mut accounts = self.accounts.write().await;

        // ── Validate (immutable borrows scoped to this block) ─────────────────
        {
            let debit_account = accounts
                .get(&debit_id)
                .ok_or_else(|| BlazerError::NotFound {
                    resource: "Account".to_owned(),
                    id: debit_id.to_string(),
                })?;
            let credit_account = accounts
                .get(&credit_id)
                .ok_or_else(|| BlazerError::NotFound {
                    resource: "Account".to_owned(),
                    id: credit_id.to_string(),
                })?;
            double_entry::validate_transfer(&transfer, debit_account, credit_account)?;
        }

        // ── Apply balance updates ─────────────────────────────────────────────
        let amount = transfer.amount().clone();

        accounts
            .get_mut(&debit_id)
            .ok_or_else(|| BlazerError::Internal("debit account vanished under lock".to_owned()))?
            .apply_debit(amount.clone())?;

        accounts
            .get_mut(&credit_id)
            .ok_or_else(|| BlazerError::Internal("credit account vanished under lock".to_owned()))?
            .apply_credit(amount)?;

        drop(accounts);

        // ── Record the transfer ───────────────────────────────────────────────
        let mut transfers = self.transfers.write().await;
        transfers.insert(transfer_id, transfer);

        tracing::info!(transfer_id = %transfer_id, "transfer committed");
        Ok(transfer_id)
    }

    /// Returns a clone of the account, or [`BlazerError::NotFound`].
    #[instrument(skip(self))]
    async fn get_account(&self, id: &AccountId) -> BlazerResult<Account> {
        self.accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| BlazerError::NotFound {
                resource: "Account".to_owned(),
                id: id.to_string(),
            })
    }

    /// Returns a clone of the transfer, or [`BlazerError::NotFound`].
    #[instrument(skip(self))]
    async fn get_transfer(&self, id: &TransferId) -> BlazerResult<Transfer> {
        self.transfers
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| BlazerError::NotFound {
                resource: "Transfer".to_owned(),
                id: id.to_string(),
            })
    }

    /// Batch-fetches accounts; silently skips missing IDs.
    #[instrument(skip(self, ids), fields(count = ids.len()))]
    async fn get_account_balances(&self, ids: &[AccountId]) -> BlazerResult<Vec<Account>> {
        let accounts = self.accounts.read().await;
        let result = ids
            .iter()
            .filter_map(|id| accounts.get(id).cloned())
            .collect();
        Ok(result)
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransferId};
    use rust_decimal::Decimal;

    use crate::account::{Account, AccountFlags};
    use crate::transfer::Transfer;

    fn usd_account() -> Account {
        Account::new(
            AccountId::new(),
            LedgerId::USD,
            parse_currency("USD").unwrap(),
            1,
            AccountFlags::default(),
        )
    }

    fn usd_account_constrained() -> Account {
        let flags = AccountFlags {
            debits_must_not_exceed_credits: true,
            ..AccountFlags::default()
        };
        Account::new(
            AccountId::new(),
            LedgerId::USD,
            parse_currency("USD").unwrap(),
            1,
            flags,
        )
    }

    fn eur_account() -> Account {
        Account::new(
            AccountId::new(),
            LedgerId::USD,
            parse_currency("EUR").unwrap(),
            1,
            AccountFlags::default(),
        )
    }

    fn usd_amount(cents: i64) -> Amount {
        Amount::new(Decimal::new(cents, 2), parse_currency("USD").unwrap()).unwrap()
    }

    async fn funded_pair(client: &InMemoryLedgerClient) -> (AccountId, AccountId) {
        // Two unconstrained accounts: debit can go negative (overdraft allowed)
        let debit = usd_account();
        let credit = usd_account();
        let d = client.create_account(debit).await.unwrap();
        let c = client.create_account(credit).await.unwrap();
        (d, c)
    }

    // ── create_account ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_two_accounts_succeeds() {
        let client = InMemoryLedgerClient::new();
        let (d, c) = funded_pair(&client).await;
        assert_eq!(client.account_count().await, 2);
        assert_ne!(d, c);
    }

    #[tokio::test]
    async fn create_duplicate_account_returns_duplicate_error() {
        let client = InMemoryLedgerClient::new();
        let account = usd_account();
        client.create_account(account.clone()).await.unwrap();
        let err = client.create_account(account).await.unwrap_err();
        assert!(matches!(err, BlazerError::Duplicate { .. }));
    }

    // ── create_transfer ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_transfer_updates_balances() {
        let client = InMemoryLedgerClient::new();
        let (debit_id, credit_id) = funded_pair(&client).await;

        let amount = usd_amount(10_000); // $100.00
        let transfer = Transfer::new(
            TransferId::new(),
            debit_id,
            credit_id,
            amount,
            LedgerId::USD,
            1,
        )
        .unwrap();
        client.create_transfer(transfer).await.unwrap();
        assert_eq!(client.transfer_count().await, 1);

        let debit_acct = client.get_account(&debit_id).await.unwrap();
        let credit_acct = client.get_account(&credit_id).await.unwrap();

        assert_eq!(debit_acct.debits_posted().value(), Decimal::new(10_000, 2));
        assert_eq!(
            credit_acct.credits_posted().value(),
            Decimal::new(10_000, 2)
        );
    }

    #[tokio::test]
    async fn create_transfer_insufficient_funds_returns_error() {
        let client = InMemoryLedgerClient::new();
        let constrained = usd_account_constrained();
        let credit = usd_account();
        let debit_id = client.create_account(constrained).await.unwrap();
        let credit_id = client.create_account(credit).await.unwrap();

        let transfer = Transfer::new(
            TransferId::new(),
            debit_id,
            credit_id,
            usd_amount(1_000),
            LedgerId::USD,
            1,
        )
        .unwrap();
        let err = client.create_transfer(transfer).await.unwrap_err();
        assert!(matches!(err, BlazerError::InsufficientFunds { .. }));
    }

    #[tokio::test]
    async fn create_transfer_currency_mismatch_returns_error() {
        let client = InMemoryLedgerClient::new();
        let debit = usd_account();
        let credit = eur_account();
        let debit_id = client.create_account(debit).await.unwrap();
        let credit_id = client.create_account(credit).await.unwrap();

        // Transfer with USD amount against an EUR credit account
        let usd_transfer = Transfer::new(
            TransferId::new(),
            debit_id,
            credit_id,
            usd_amount(1_000),
            LedgerId::USD,
            1,
        )
        .unwrap();
        let err = client.create_transfer(usd_transfer).await.unwrap_err();
        assert!(matches!(err, BlazerError::CurrencyMismatch { .. }));
    }

    // ── get_account ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_nonexistent_account_returns_not_found() {
        let client = InMemoryLedgerClient::new();
        let fake_id = AccountId::new();
        let err = client.get_account(&fake_id).await.unwrap_err();
        assert!(matches!(err, BlazerError::NotFound { .. }));
    }

    #[tokio::test]
    async fn get_account_after_create_returns_correct_account() {
        let client = InMemoryLedgerClient::new();
        let account = usd_account();
        let id = *account.id();
        client.create_account(account).await.unwrap();
        let fetched = client.get_account(&id).await.unwrap();
        assert_eq!(fetched.id(), &id);
    }

    // ── get_transfer ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_nonexistent_transfer_returns_not_found() {
        let client = InMemoryLedgerClient::new();
        let err = client.get_transfer(&TransferId::new()).await.unwrap_err();
        assert!(matches!(err, BlazerError::NotFound { .. }));
    }

    // ── get_account_balances ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_account_balances_returns_correct_results() {
        let client = InMemoryLedgerClient::new();
        let (d, c) = funded_pair(&client).await;
        let other = AccountId::new(); // does not exist

        let results = client.get_account_balances(&[d, c, other]).await.unwrap();
        assert_eq!(results.len(), 2, "missing ID should be silently skipped");
    }

    #[tokio::test]
    async fn get_account_balances_empty_input_returns_empty() {
        let client = InMemoryLedgerClient::new();
        let results = client.get_account_balances(&[]).await.unwrap();
        assert!(results.is_empty());
    }
}

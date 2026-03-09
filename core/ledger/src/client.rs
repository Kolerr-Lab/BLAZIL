//! Abstract `LedgerClient` trait.
//!
//! All business logic in Blazil depends on this trait, never on a concrete
//! client implementation. This means the entire ledger layer is testable
//! in isolation using [`crate::mock::InMemoryLedgerClient`] without requiring
//! a running TigerBeetle instance.
//!
//! # Implementations
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`crate::mock::InMemoryLedgerClient`] | In-memory, used in all tests |
//! | `TigerBeetleClient` (feature `tigerbeetle-client`) | Production, connects to TigerBeetle |
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::client::LedgerClient;
//! use blazil_ledger::mock::InMemoryLedgerClient;
//! use blazil_ledger::account::{Account, AccountFlags};
//! use blazil_common::ids::{AccountId, LedgerId};
//! use blazil_common::currency::parse_currency;
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = InMemoryLedgerClient::new();
//!     let usd = parse_currency("USD").unwrap();
//!     let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default());
//!     let id = client.create_account(account).await.unwrap();
//!     let fetched = client.get_account(&id).await.unwrap();
//!     assert_eq!(fetched.id(), &id);
//! }
//! ```

use async_trait::async_trait;
use blazil_common::error::BlazerResult;
use blazil_common::ids::{AccountId, TransferId};

use crate::account::Account;
use crate::transfer::Transfer;

/// The abstract interface for all ledger I/O.
///
/// Business logic depends exclusively on this trait. Concrete implementations
/// are injected at the call site, enabling easy substitution of the mock for
/// production code in tests.
///
/// All methods are `async` to support both the in-memory mock (which resolves
/// immediately) and the real TigerBeetle client (which involves network I/O).
#[async_trait]
pub trait LedgerClient: Send + Sync {
    /// Creates a new account in the ledger.
    ///
    /// Returns the [`AccountId`] of the newly created account.
    ///
    /// # Errors
    ///
    /// - [`blazil_common::error::BlazerError::Duplicate`] if an account with
    ///   the same ID already exists.
    /// - [`blazil_common::error::BlazerError::ValidationError`] if the account
    ///   fails structural validation.
    /// - [`blazil_common::error::BlazerError::Ledger`] on TigerBeetle errors.
    async fn create_account(&self, account: Account) -> BlazerResult<AccountId>;

    /// Creates a transfer between two accounts.
    ///
    /// This is the atomic unit of all money movement. The transfer is validated
    /// (currency, funds) before being submitted to the ledger.
    ///
    /// Returns the [`TransferId`] of the committed transfer.
    ///
    /// # Errors
    ///
    /// - [`blazil_common::error::BlazerError::CurrencyMismatch`] if currencies
    ///   do not match.
    /// - [`blazil_common::error::BlazerError::InsufficientFunds`] if the debit
    ///   account cannot cover the transfer (when constrained).
    /// - [`blazil_common::error::BlazerError::NotFound`] if either account
    ///   does not exist.
    /// - [`blazil_common::error::BlazerError::Ledger`] on TigerBeetle errors.
    async fn create_transfer(&self, transfer: Transfer) -> BlazerResult<TransferId>;

    /// Looks up an account by its [`AccountId`].
    ///
    /// # Errors
    ///
    /// - [`blazil_common::error::BlazerError::NotFound`] if no account with
    ///   this ID exists.
    async fn get_account(&self, id: &AccountId) -> BlazerResult<Account>;

    /// Looks up a transfer by its [`TransferId`].
    ///
    /// # Errors
    ///
    /// - [`blazil_common::error::BlazerError::NotFound`] if no transfer with
    ///   this ID exists.
    async fn get_transfer(&self, id: &TransferId) -> BlazerResult<Transfer>;

    /// Batch-looks up accounts by a slice of [`AccountId`]s.
    ///
    /// More efficient than calling [`get_account`] in a loop. Missing IDs are
    /// silently skipped — the returned `Vec` may be shorter than `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`blazil_common::error::BlazerError::Ledger`] only on
    /// unrecoverable transport errors; individual missing IDs are skipped.
    ///
    /// [`get_account`]: LedgerClient::get_account
    async fn get_account_balances(&self, ids: &[AccountId]) -> BlazerResult<Vec<Account>>;
}

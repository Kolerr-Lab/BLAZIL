//! TigerBeetle [`LedgerClient`] implementation.
//!
//! This module connects directly to a running TigerBeetle cluster and
//! translates between Blazil domain types and TigerBeetle wire types via
//! [`crate::convert`].
//!
//! # Feature gate
//!
//! This module is only compiled when the `tigerbeetle-client` feature is
//! enabled:
//!
//! ```toml
//! blazil-ledger = { path = "../ledger", features = ["tigerbeetle-client"] }
//! ```
//!
//! This prevents the TigerBeetle C library (Zig-based build) from being
//! compiled on machines that only need to run tests with the mock.
//!
//! # Architectural flag
//!
//! The feature-gating of `TigerBeetleClient` is a deviation from a literal
//! reading of the spec. It was made to satisfy the quality gate
//! `cargo check --workspace` on machines without Zig installed.
//! **Flag for Architecture Room review.**

use async_trait::async_trait;
use std::time::Instant;
use tracing::instrument;

use blazil_common::currency::Currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, TransferId};

use crate::account::{Account, AccountFlags};
use crate::client::LedgerClient;
use crate::convert;
use crate::transfer::Transfer;

// Pull in the TB re-exports from the crate root
use tigerbeetle_unofficial as tb;

// ── TigerBeetleClient ─────────────────────────────────────────────────────────

/// A [`LedgerClient`] backed by a live TigerBeetle cluster.
///
/// Connects over TCP to a TigerBeetle server at construction time.
/// All methods translate Blazil types → TigerBeetle wire types, submit the
/// request, and map any errors to [`BlazerError`].
///
/// # Examples
///
/// ```rust,no_run
/// use blazil_ledger::tigerbeetle::TigerBeetleClient;
///
/// #[tokio::main]
/// async fn main() {
///     let client = TigerBeetleClient::connect("127.0.0.1:3000", 0).await.unwrap();
/// }
/// ```
pub struct TigerBeetleClient {
    inner: tb::Client,
    address: String,
}

impl TigerBeetleClient {
    /// Connects to TigerBeetle at `address` for the given `cluster_id`.
    ///
    /// `address` is typically `"127.0.0.1:3000"` for a local single-node
    /// cluster.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Ledger`] if the connection cannot be established.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use blazil_ledger::tigerbeetle::TigerBeetleClient;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let client = TigerBeetleClient::connect("127.0.0.1:3000", 0).await.unwrap();
    /// }
    /// ```
    pub async fn connect(address: &str, cluster_id: u128) -> BlazerResult<Self> {
        let inner = tb::Client::new(cluster_id, address)
            .map_err(|e| BlazerError::Ledger(format!("TigerBeetle connect failed: {e}")))?;
        Ok(Self {
            inner,
            address: address.to_owned(),
        })
    }

    /// Returns the address this client is connected to.
    pub fn address(&self) -> &str {
        &self.address
    }
}

// ── LedgerClient impl ─────────────────────────────────────────────────────────

#[async_trait]
impl LedgerClient for TigerBeetleClient {
    #[instrument(skip(self, account), fields(account_id = %account.id()))]
    async fn create_account(&self, account: Account) -> BlazerResult<AccountId> {
        let id = *account.id();
        tracing::debug!(account_id = %id, "submitting create_account to TigerBeetle");

        let tb_id = convert::account_id_to_u128(&id);
        let ledger = convert::ledger_id_to_u32(account.ledger_id());

        let mut flags = tb::account::Flags::empty();
        if account.flags().debits_must_not_exceed_credits {
            flags.insert(tb::account::Flags::DEBITS_MUST_NOT_EXCEED_CREDITS);
        }
        if account.flags().credits_must_not_exceed_debits {
            flags.insert(tb::account::Flags::CREDITS_MUST_NOT_EXCEED_DEBITS);
        }
        if account.flags().linked {
            flags.insert(tb::account::Flags::LINKED);
        }

        let tb_account = tb::Account::new(tb_id, ledger, account.code())
            .with_flags(flags)
            .with_user_data_32(u32::from(account.currency().numeric()));

        let t0 = Instant::now();
        self.inner
            .create_accounts(vec![tb_account])
            .await
            .map_err(|e| BlazerError::Ledger(format!("create_accounts failed: {e}")))?;
        tracing::info!(account_id = %id, elapsed_ms = t0.elapsed().as_millis(), "account created in TigerBeetle");
        Ok(id)
    }

    #[instrument(skip(self, transfer), fields(transfer_id = %transfer.id()))]
    async fn create_transfer(&self, transfer: Transfer) -> BlazerResult<TransferId> {
        let transfer_id = *transfer.id();
        tracing::debug!(transfer_id = %transfer_id, "submitting create_transfer to TigerBeetle");

        let tb_id = convert::transfer_id_to_u128(&transfer_id);
        let debit_id = convert::account_id_to_u128(transfer.debit_account_id());
        let credit_id = convert::account_id_to_u128(transfer.credit_account_id());
        let ledger = convert::ledger_id_to_u32(transfer.ledger_id());
        let amount = convert::amount_to_minor_units(transfer.amount())?;

        let mut flags = tb::transfer::Flags::empty();
        if transfer.flags().linked {
            flags.insert(tb::transfer::Flags::LINKED);
        }
        if transfer.flags().pending {
            flags.insert(tb::transfer::Flags::PENDING);
        }
        if transfer.flags().post_pending_transfer {
            flags.insert(tb::transfer::Flags::POST_PENDING_TRANSFER);
        }
        if transfer.flags().void_pending_transfer {
            flags.insert(tb::transfer::Flags::VOID_PENDING_TRANSFER);
        }

        let tb_transfer = tb::Transfer::new(tb_id)
            .with_debit_account_id(debit_id)
            .with_credit_account_id(credit_id)
            .with_ledger(ledger)
            .with_code(transfer.code())
            .with_amount(amount)
            .with_flags(flags)
            .with_user_data_32(u32::from(transfer.amount().currency().numeric()));

        let t0 = Instant::now();
        self.inner
            .create_transfers(vec![tb_transfer])
            .await
            .map_err(|e| BlazerError::Ledger(format!("create_transfers failed: {e}")))?;
        tracing::info!(transfer_id = %transfer_id, elapsed_ms = t0.elapsed().as_millis(), "transfer committed to TigerBeetle");
        Ok(transfer_id)
    }

    #[instrument(skip(self))]
    async fn get_account(&self, id: &AccountId) -> BlazerResult<Account> {
        tracing::debug!(account_id = %id, "looking up account in TigerBeetle");

        let tb_id = convert::account_id_to_u128(id);
        let t0 = Instant::now();
        let mut results = self
            .inner
            .lookup_accounts(vec![tb_id])
            .await
            .map_err(|e| BlazerError::Ledger(format!("lookup_accounts failed: {e}")))?;
        tracing::debug!(account_id = %id, elapsed_ms = t0.elapsed().as_millis(), "lookup_accounts completed");

        let tb_account = results.pop().ok_or_else(|| BlazerError::NotFound {
            resource: "Account".to_owned(),
            id: id.to_string(),
        })?;

        tb_account_to_blazil(tb_account)
    }

    #[instrument(skip(self))]
    async fn get_transfer(&self, id: &TransferId) -> BlazerResult<Transfer> {
        tracing::debug!(transfer_id = %id, "looking up transfer in TigerBeetle");

        let tb_id = convert::transfer_id_to_u128(id);
        let t0 = Instant::now();
        let mut results = self
            .inner
            .lookup_transfers(vec![tb_id])
            .await
            .map_err(|e| BlazerError::Ledger(format!("lookup_transfers failed: {e}")))?;
        tracing::debug!(transfer_id = %id, elapsed_ms = t0.elapsed().as_millis(), "lookup_transfers completed");

        let tb_transfer = results.pop().ok_or_else(|| BlazerError::NotFound {
            resource: "Transfer".to_owned(),
            id: id.to_string(),
        })?;

        tb_transfer_to_blazil(tb_transfer)
    }

    #[instrument(skip(self, ids), fields(count = ids.len()))]
    async fn get_account_balances(&self, ids: &[AccountId]) -> BlazerResult<Vec<Account>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let tb_ids: Vec<u128> = ids.iter().map(convert::account_id_to_u128).collect();
        let t0 = Instant::now();
        let tb_accounts = self
            .inner
            .lookup_accounts(tb_ids)
            .await
            .map_err(|e| BlazerError::Ledger(format!("lookup_accounts (batch) failed: {e}")))?;
        tracing::debug!(
            count = ids.len(),
            elapsed_ms = t0.elapsed().as_millis(),
            "batch lookup_accounts completed"
        );

        tb_accounts
            .into_iter()
            .map(tb_account_to_blazil)
            .collect::<BlazerResult<Vec<_>>>()
    }
}

// ── Internal conversion helpers ───────────────────────────────────────────────

/// Converts a TigerBeetle [`tb::Account`] back to a Blazil [`Account`].
///
/// The currency code is stored in `user_data_32` as the ISO 4217 numeric code.
/// If the currency is unknown, the conversion fails with [`BlazerError::InvalidCurrency`].
fn tb_account_to_blazil(tb: tb::Account) -> BlazerResult<Account> {
    let id = convert::u128_to_account_id(tb.id());
    // Currency is stored using the ISO 4217 numeric code in user_data_32
    let numeric = tb.user_data_32();
    let currency =
        Currency::from_numeric(u16::try_from(numeric).unwrap_or(0)).ok_or_else(|| {
            BlazerError::InvalidCurrency(format!("unknown numeric code: {}", numeric))
        })?;

    // Reconstruct LedgerId from the u32 ledger field
    let ledger_id = blazil_common::ids::LedgerId::new(tb.ledger())?;

    let tb_flags = tb.flags();
    let flags = AccountFlags {
        debits_must_not_exceed_credits: tb_flags
            .contains(tb::account::Flags::DEBITS_MUST_NOT_EXCEED_CREDITS),
        credits_must_not_exceed_debits: tb_flags
            .contains(tb::account::Flags::CREDITS_MUST_NOT_EXCEED_DEBITS),
        linked: tb_flags.contains(tb::account::Flags::LINKED),
    };

    // Create account with zero balances, then apply the posted amounts
    let mut account = Account::new(id, ledger_id, currency.clone(), tb.code(), flags);

    let debits_minor = tb.debits_posted();
    let credits_minor = tb.credits_posted();

    if debits_minor > 0 {
        let debits_posted = convert::minor_units_to_amount(debits_minor, currency.clone())?;
        account.apply_debit(debits_posted)?;
    }
    if credits_minor > 0 {
        let credits_posted = convert::minor_units_to_amount(credits_minor, currency.clone())?;
        account.apply_credit(credits_posted)?;
    }

    Ok(account)
}

/// Converts a TigerBeetle [`tb::Transfer`] back to a Blazil [`Transfer`].
fn tb_transfer_to_blazil(tb: tb::Transfer) -> BlazerResult<Transfer> {
    let debit_id = convert::u128_to_account_id(tb.debit_account_id());
    let credit_id = convert::u128_to_account_id(tb.credit_account_id());
    let transfer_id = convert::u128_to_transfer_id(tb.id());

    let ledger_id = blazil_common::ids::LedgerId::new(tb.ledger())?;

    // Currency is not stored on the transfer itself in TigerBeetle.
    // We use user_data_32 to store it (same convention as on Account).
    let numeric = tb.user_data_32();
    let currency =
        Currency::from_numeric(u16::try_from(numeric).unwrap_or(0)).ok_or_else(|| {
            BlazerError::InvalidCurrency(format!("unknown numeric code: {}", numeric))
        })?;

    let amount = convert::minor_units_to_amount(tb.amount(), currency)?;

    let tf_flags = tb.flags();
    let flags = crate::transfer::TransferFlags {
        linked: tf_flags.contains(tb::transfer::Flags::LINKED),
        pending: tf_flags.contains(tb::transfer::Flags::PENDING),
        post_pending_transfer: tf_flags.contains(tb::transfer::Flags::POST_PENDING_TRANSFER),
        void_pending_transfer: tf_flags.contains(tb::transfer::Flags::VOID_PENDING_TRANSFER),
    };

    Ok(Transfer::from_ledger(
        transfer_id,
        debit_id,
        credit_id,
        amount,
        ledger_id,
        tb.code(),
        flags,
    ))
}

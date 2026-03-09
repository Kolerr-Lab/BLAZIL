//! Transfer domain model.
//!
//! A [`Transfer`] is the atomic unit of all money movement in Blazil.
//! Every transfer debits exactly one account and credits exactly one other
//! account. The two amounts are always equal — TigerBeetle enforces
//! double-entry accounting at the database level.
//!
//! # Construction
//!
//! Use [`Transfer::new`] which validates the transfer before returning it.
//! Validation catches self-transfers and zero-amount transfers immediately,
//! before any ledger I/O is attempted.
//!
//! Full contextual validation (currency mismatch, insufficient funds) happens
//! in [`crate::double_entry::validate_transfer`] which additionally requires
//! the two [`crate::account::Account`] objects.
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::transfer::{Transfer, TransferFlags};
//! use blazil_common::ids::{AccountId, LedgerId, TransferId};
//! use blazil_common::amount::Amount;
//! use blazil_common::currency::parse_currency;
//! use rust_decimal::Decimal;
//!
//! let usd = parse_currency("USD").unwrap();
//! let amount = Amount::new(Decimal::new(10_000, 2), usd).unwrap(); // $100.00
//! let transfer = Transfer::new(
//!     TransferId::new(),
//!     AccountId::new(),
//!     AccountId::new(),
//!     amount,
//!     LedgerId::USD,
//!     1,
//! ).unwrap();
//! ```

use blazil_common::amount::Amount;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransferId};
use blazil_common::timestamp::Timestamp;
use blazil_common::traits::Validate;

// ── TransferFlags ─────────────────────────────────────────────────────────────

/// Behavioural flags that control how TigerBeetle processes this transfer.
///
/// All flags default to `false` (a standard, immediately-posted transfer).
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::transfer::TransferFlags;
///
/// let flags = TransferFlags { pending: true, ..TransferFlags::default() };
/// assert!(flags.pending);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransferFlags {
    /// This transfer is atomically linked with the next transfer in a batch.
    pub linked: bool,
    /// This transfer is a two-phase (pending) transfer. Funds are reserved but
    /// not yet posted. A subsequent `post_pending_transfer` finalises it.
    pub pending: bool,
    /// Posts a previously created pending transfer.
    pub post_pending_transfer: bool,
    /// Voids a previously created pending transfer, releasing the reserved funds.
    pub void_pending_transfer: bool,
}

// ── Transfer ──────────────────────────────────────────────────────────────────

/// A financial transfer between two accounts.
///
/// Represents an atomic unit of money movement: `amount` leaves
/// `debit_account_id` and arrives at `credit_account_id`.
///
/// # Invariants (enforced at construction)
///
/// - `id` must not be the nil UUID.
/// - `debit_account_id` ≠ `credit_account_id` (no self-transfers).
/// - `amount.value()` must be strictly positive.
/// - `amount.value().scale()` must be ≤ 8.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::transfer::Transfer;
/// use blazil_common::ids::{AccountId, LedgerId, TransferId};
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let amount = Amount::new(Decimal::new(5_00, 2), usd).unwrap(); // $5.00
/// let transfer = Transfer::new(
///     TransferId::new(),
///     AccountId::new(),
///     AccountId::new(),
///     amount,
///     LedgerId::USD,
///     1,
/// ).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct Transfer {
    id: TransferId,
    debit_account_id: AccountId,
    credit_account_id: AccountId,
    amount: Amount,
    ledger_id: LedgerId,
    code: u16,
    flags: TransferFlags,
    timestamp: Timestamp,
}

impl Transfer {
    /// Creates a validated transfer.
    ///
    /// Calls [`Transfer::validate`] before returning. Any invariant violation
    /// returns a [`BlazerError`] immediately.
    ///
    /// Flags default to [`TransferFlags::default`] (immediate, non-pending).
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::ValidationError`] if the transfer violates any
    /// structural invariant.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::transfer::Transfer;
    /// use blazil_common::ids::{AccountId, LedgerId, TransferId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let amount = Amount::new(Decimal::new(1_000, 2), usd).unwrap();
    /// let result = Transfer::new(
    ///     TransferId::new(), AccountId::new(), AccountId::new(),
    ///     amount, LedgerId::USD, 1,
    /// );
    /// assert!(result.is_ok());
    /// ```
    pub fn new(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        ledger_id: LedgerId,
        code: u16,
    ) -> BlazerResult<Self> {
        let transfer = Self {
            id,
            debit_account_id,
            credit_account_id,
            amount,
            ledger_id,
            code,
            flags: TransferFlags::default(),
            timestamp: Timestamp::now(),
        };
        transfer.validate()?;
        Ok(transfer)
    }

    /// Creates a transfer **without** validation. Only for internal test helpers
    /// that need to construct invalid transfers (e.g. self-transfers) to verify
    /// that downstream validators reject them correctly.
    #[cfg(test)]
    pub(crate) fn new_unchecked(
        id: TransferId,
        debit_account_id: AccountId,
        credit_account_id: AccountId,
        amount: Amount,
        ledger_id: LedgerId,
        code: u16,
    ) -> Self {
        Self {
            id,
            debit_account_id,
            credit_account_id,
            amount,
            ledger_id,
            code,
            flags: TransferFlags::default(),
            timestamp: Timestamp::now(),
        }
    }

    /// The unique identifier of this transfer.
    pub fn id(&self) -> &TransferId {
        &self.id
    }

    /// The account that is debited (money leaves).
    pub fn debit_account_id(&self) -> &AccountId {
        &self.debit_account_id
    }

    /// The account that is credited (money arrives).
    pub fn credit_account_id(&self) -> &AccountId {
        &self.credit_account_id
    }

    /// The monetary amount being transferred.
    pub fn amount(&self) -> &Amount {
        &self.amount
    }

    /// The ledger this transfer is recorded on.
    pub fn ledger_id(&self) -> &LedgerId {
        &self.ledger_id
    }

    /// The TigerBeetle transfer code (carries business meaning).
    pub fn code(&self) -> u16 {
        self.code
    }

    /// The behavioural flags for this transfer.
    pub fn flags(&self) -> &TransferFlags {
        &self.flags
    }

    /// The timestamp at which this Transfer struct was created locally.
    pub fn timestamp(&self) -> &Timestamp {
        &self.timestamp
    }
}

impl Validate for Transfer {
    /// Validates the transfer's structural invariants.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::ValidationError`] if `id` is the nil UUID.
    /// - [`BlazerError::ValidationError`] if `debit_account_id == credit_account_id`.
    /// - [`BlazerError::ValidationError`] if `amount.value()` is zero.
    /// - [`BlazerError::InvalidAmountScale`] if `amount.value().scale() > 8`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::transfer::Transfer;
    /// use blazil_common::ids::{AccountId, LedgerId, TransferId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use blazil_common::traits::Validate;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let amount = Amount::new(Decimal::new(1_000, 2), usd).unwrap();
    /// let transfer = Transfer::new(
    ///     TransferId::new(), AccountId::new(), AccountId::new(),
    ///     amount, LedgerId::USD, 1,
    /// ).unwrap();
    /// assert!(transfer.validate().is_ok());
    /// ```
    fn validate(&self) -> BlazerResult<()> {
        if self.id.as_uuid().is_nil() {
            return Err(BlazerError::ValidationError(
                "transfer id must not be the nil UUID".to_owned(),
            ));
        }
        if self.debit_account_id == self.credit_account_id {
            return Err(BlazerError::ValidationError(
                "debit and credit accounts must be different (no self-transfers)".to_owned(),
            ));
        }
        if self.amount.value().is_zero() {
            return Err(BlazerError::ValidationError(
                "transfer amount must be greater than zero".to_owned(),
            ));
        }
        if self.amount.value().scale() > 8 {
            return Err(BlazerError::InvalidAmountScale(self.amount.value().scale()));
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, LedgerId, TransferId};
    use blazil_common::traits::Validate;
    use rust_decimal::Decimal;

    fn usd_amount(cents: i64) -> Amount {
        Amount::new(Decimal::new(cents, 2), parse_currency("USD").unwrap()).unwrap()
    }

    fn valid_transfer() -> Transfer {
        Transfer::new(
            TransferId::new(),
            AccountId::new(),
            AccountId::new(),
            usd_amount(10_000),
            LedgerId::USD,
            1,
        )
        .unwrap()
    }

    #[test]
    fn new_succeeds_with_valid_inputs() {
        let t = valid_transfer();
        assert_eq!(t.code(), 1);
        assert!(!t.id().as_uuid().is_nil());
    }

    #[test]
    fn new_rejects_self_transfer() {
        let account_id = AccountId::new();
        let err = Transfer::new(
            TransferId::new(),
            account_id,
            account_id,
            usd_amount(1_000),
            LedgerId::USD,
            1,
        )
        .unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn new_rejects_zero_amount() {
        // Amount::new rejects zero Decimal. Amount::zero() produces a zero Amount.
        // Use new_unchecked to bypass Transfer::new's self-validate so we can test
        // that validate() independently rejects a zero amount.
        let usd = parse_currency("USD").unwrap();
        let zero_amount = Amount::zero(usd);
        let t_unchecked = Transfer::new_unchecked(
            TransferId::new(),
            AccountId::new(),
            AccountId::new(),
            zero_amount,
            LedgerId::USD,
            1,
        );
        let err = t_unchecked.validate().unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn validate_fails_on_nil_transfer_id() {
        let t = Transfer::new_unchecked(
            TransferId::from_bytes([0u8; 16]),
            AccountId::new(),
            AccountId::new(),
            usd_amount(1_000),
            LedgerId::USD,
            1,
        );
        let err = t.validate().unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn valid_transfer_passes_validate() {
        assert!(valid_transfer().validate().is_ok());
    }

    #[test]
    fn transfer_getters_return_correct_values() {
        let debit_id = AccountId::new();
        let credit_id = AccountId::new();
        let ledger_id = LedgerId::USD;
        let amount = usd_amount(5_00);

        let t = Transfer::new(
            TransferId::new(),
            debit_id,
            credit_id,
            amount,
            ledger_id,
            42,
        )
        .unwrap();

        assert_eq!(t.debit_account_id(), &debit_id);
        assert_eq!(t.credit_account_id(), &credit_id);
        assert_eq!(t.ledger_id(), &ledger_id);
        assert_eq!(t.code(), 42);
    }
}

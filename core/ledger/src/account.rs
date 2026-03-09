//! Financial account domain model.
//!
//! An [`Account`] represents a single ledger account in the Blazil system.
//! Every account tracks `debits_posted` and `credits_posted` separately;
//! the net balance is `credits_posted − debits_posted`.
//!
//! # Double-entry invariant
//!
//! Blazil never stores a "balance" directly. Instead it stores gross debits
//! and gross credits and derives the balance on demand. This mirrors how
//! TigerBeetle stores account state and prevents balance corruption from
//! partial updates.
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::account::{Account, AccountFlags};
//! use blazil_common::ids::{AccountId, LedgerId};
//! use blazil_common::currency::parse_currency;
//!
//! let usd = parse_currency("USD").unwrap();
//! let account = Account::new(
//!     AccountId::new(),
//!     LedgerId::USD,
//!     usd,
//!     1,
//!     AccountFlags::default(),
//! );
//! assert_eq!(account.balance().unwrap().value(), rust_decimal::Decimal::ZERO);
//! ```

use blazil_common::amount::Amount;
use blazil_common::currency::Currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId};
use blazil_common::timestamp::Timestamp;
use blazil_common::traits::Validate;

// ── AccountFlags ──────────────────────────────────────────────────────────────

/// Behavioural flags that govern how TigerBeetle enforces account constraints.
///
/// All flags default to `false` (permissive / unconstrained).
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::account::AccountFlags;
///
/// let flags = AccountFlags {
///     debits_must_not_exceed_credits: true,
///     ..AccountFlags::default()
/// };
/// assert!(flags.debits_must_not_exceed_credits);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountFlags {
    /// Prevents the account's debits from exceeding its credits.
    /// Effectively enforces that the account cannot go into overdraft.
    pub debits_must_not_exceed_credits: bool,
    /// Prevents the account's credits from exceeding its debits.
    /// Useful for liability accounts that must not over-receive.
    pub credits_must_not_exceed_debits: bool,
    /// When `true`, this account is atomically linked with the next account in
    /// a batch — either both are created or neither is.
    pub linked: bool,
}

// ── Account ───────────────────────────────────────────────────────────────────

/// A financial account in the Blazil ledger.
///
/// Tracks gross debits and gross credits separately. The net balance is derived
/// from `credits_posted − debits_posted`. Every account belongs to a single
/// [`LedgerId`] and holds a single [`Currency`].
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::account::{Account, AccountFlags};
/// use blazil_common::ids::{AccountId, LedgerId};
/// use blazil_common::currency::parse_currency;
///
/// let usd = parse_currency("USD").unwrap();
/// let account = Account::new(
///     AccountId::new(),
///     LedgerId::USD,
///     usd,
///     1,
///     AccountFlags::default(),
/// );
/// assert!(account.can_debit(&blazil_common::amount::Amount::zero(parse_currency("USD").unwrap())));
/// ```
#[derive(Debug, Clone)]
pub struct Account {
    id: AccountId,
    ledger_id: LedgerId,
    currency: Currency,
    code: u16,
    flags: AccountFlags,
    debits_posted: Amount,
    credits_posted: Amount,
    timestamp: Timestamp,
}

impl Account {
    /// Creates a new account with zero balances and the current timestamp.
    ///
    /// Both `debits_posted` and `credits_posted` are initialised to
    /// `Amount::zero(currency)`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::account::{Account, AccountFlags};
    /// use blazil_common::ids::{AccountId, LedgerId};
    /// use blazil_common::currency::parse_currency;
    ///
    /// let eur = parse_currency("EUR").unwrap();
    /// let account = Account::new(AccountId::new(), LedgerId::USD, eur, 1, AccountFlags::default());
    /// ```
    pub fn new(
        id: AccountId,
        ledger_id: LedgerId,
        currency: Currency,
        code: u16,
        flags: AccountFlags,
    ) -> Self {
        Self {
            debits_posted: Amount::zero(currency),
            credits_posted: Amount::zero(currency),
            id,
            ledger_id,
            currency,
            code,
            flags,
            timestamp: Timestamp::now(),
        }
    }

    /// The unique identifier of this account.
    pub fn id(&self) -> &AccountId {
        &self.id
    }

    /// The ledger this account belongs to.
    pub fn ledger_id(&self) -> &LedgerId {
        &self.ledger_id
    }

    /// The ISO 4217 currency this account holds.
    pub fn currency(&self) -> &Currency {
        &self.currency
    }

    /// The TigerBeetle account code (carries business meaning, e.g. asset=1).
    pub fn code(&self) -> u16 {
        self.code
    }

    /// The behavioural flags for this account.
    pub fn flags(&self) -> &AccountFlags {
        &self.flags
    }

    /// The total amount debited from this account (gross, not net).
    pub fn debits_posted(&self) -> &Amount {
        &self.debits_posted
    }

    /// The total amount credited to this account (gross, not net).
    pub fn credits_posted(&self) -> &Amount {
        &self.credits_posted
    }

    /// The creation timestamp of this account record.
    pub fn timestamp(&self) -> &Timestamp {
        &self.timestamp
    }

    /// Returns the net balance: `credits_posted − debits_posted`.
    ///
    /// Returns [`BlazerError::AmountOverflow`] if the subtraction fails
    /// (e.g. debits exceed credits on an unconstrained account, producing a
    /// conceptually negative balance that cannot be represented as an
    /// non-negative [`Amount`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::account::{Account, AccountFlags};
    /// use blazil_common::ids::{AccountId, LedgerId};
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default());
    /// assert_eq!(account.balance().unwrap().value(), Decimal::ZERO);
    /// ```
    pub fn balance(&self) -> BlazerResult<Amount> {
        self.credits_posted
            .clone()
            .checked_sub(self.debits_posted.clone())
            .map_err(|_| BlazerError::AmountOverflow)
    }

    /// Returns `true` if a debit of `amount` is permitted on this account.
    ///
    /// When [`AccountFlags::debits_must_not_exceed_credits`] is set, the
    /// current balance must be ≥ `amount`. Otherwise overdraft is permitted
    /// and this always returns `true`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::account::{Account, AccountFlags};
    /// use blazil_common::ids::{AccountId, LedgerId};
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let flags = AccountFlags { debits_must_not_exceed_credits: true, ..AccountFlags::default() };
    /// let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, flags);
    /// let large = Amount::new(Decimal::new(100_000, 2), parse_currency("USD").unwrap()).unwrap();
    /// assert!(!account.can_debit(&large)); // zero balance, debit not allowed
    /// ```
    pub fn can_debit(&self, amount: &Amount) -> bool {
        if self.flags.debits_must_not_exceed_credits {
            match self.balance() {
                Ok(bal) => bal.value() >= amount.value(),
                Err(_) => false,
            }
        } else {
            true
        }
    }

    /// Applies a debit to this account (internal use only).
    pub(crate) fn apply_debit(&mut self, amount: Amount) -> BlazerResult<()> {
        self.debits_posted = self.debits_posted.clone().checked_add(amount)?;
        Ok(())
    }

    /// Applies a credit to this account (internal use only).
    pub(crate) fn apply_credit(&mut self, amount: Amount) -> BlazerResult<()> {
        self.credits_posted = self.credits_posted.clone().checked_add(amount)?;
        Ok(())
    }
}

impl Validate for Account {
    /// Validates the account's internal state.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::ValidationError`] if `id` is the nil UUID.
    /// - [`BlazerError::ValidationError`] if `code` is zero.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_ledger::account::{Account, AccountFlags};
    /// use blazil_common::ids::{AccountId, LedgerId};
    /// use blazil_common::currency::parse_currency;
    /// use blazil_common::traits::Validate;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default());
    /// assert!(account.validate().is_ok());
    /// ```
    fn validate(&self) -> BlazerResult<()> {
        if self.id.as_uuid().is_nil() {
            return Err(BlazerError::ValidationError(
                "account id must not be the nil UUID".to_owned(),
            ));
        }
        if self.code == 0 {
            return Err(BlazerError::ValidationError(
                "account code must be greater than 0".to_owned(),
            ));
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
    use blazil_common::ids::{AccountId, LedgerId};
    use blazil_common::traits::Validate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

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

    fn usd_amount(cents: i64) -> Amount {
        let usd = parse_currency("USD").unwrap();
        Amount::new(Decimal::new(cents, 2), usd).unwrap()
    }

    #[test]
    fn new_creates_account_with_zero_balances() {
        let account = usd_account();
        assert_eq!(account.debits_posted().value(), Decimal::ZERO);
        assert_eq!(account.credits_posted().value(), Decimal::ZERO);
    }

    #[test]
    fn balance_returns_zero_on_fresh_account() {
        let account = usd_account();
        let bal = account.balance().unwrap();
        assert_eq!(bal.value(), Decimal::ZERO);
    }

    #[test]
    fn can_debit_returns_true_when_no_constraint() {
        let account = usd_account();
        let amount = usd_amount(1000); // $10.00
        assert!(account.can_debit(&amount));
    }

    #[test]
    fn can_debit_returns_false_when_balance_insufficient_with_constraint() {
        let usd = parse_currency("USD").unwrap();
        let flags = AccountFlags {
            debits_must_not_exceed_credits: true,
            ..AccountFlags::default()
        };
        let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, flags);
        let amount = usd_amount(1000); // $10.00 — balance is $0
        assert!(!account.can_debit(&amount));
    }

    #[test]
    fn can_debit_returns_true_when_balance_sufficient_with_constraint() {
        let usd = parse_currency("USD").unwrap();
        let flags = AccountFlags {
            debits_must_not_exceed_credits: true,
            ..AccountFlags::default()
        };
        let mut account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, flags);
        // Manually credit the account to give it a balance
        account.apply_credit(usd_amount(5000)).unwrap(); // $50.00
        let debit_amount = usd_amount(3000); // $30.00 — within balance
        assert!(account.can_debit(&debit_amount));
    }

    #[test]
    fn validate_fails_on_nil_id() {
        let usd = parse_currency("USD").unwrap();
        // Construct an account with a nil UUID via from_bytes
        let nil_id = AccountId::from_bytes([0u8; 16]);
        let account = Account {
            id: nil_id,
            ledger_id: LedgerId::USD,
            currency: usd.clone(),
            code: 1,
            flags: AccountFlags::default(),
            debits_posted: Amount::zero(usd.clone()),
            credits_posted: Amount::zero(usd),
            timestamp: Timestamp::now(),
        };
        let err = account.validate().unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn validate_fails_on_zero_code() {
        let usd = parse_currency("USD").unwrap();
        let account = Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd,
            0,
            AccountFlags::default(),
        );
        let err = account.validate().unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn validate_passes_on_valid_account() {
        let account = usd_account();
        assert!(account.validate().is_ok());
    }

    #[test]
    fn apply_debit_and_credit_update_balances() {
        let mut account = usd_account();
        account.apply_credit(usd_amount(10_000)).unwrap(); // $100.00
        account.apply_debit(usd_amount(3_000)).unwrap(); // $30.00
        let bal = account.balance().unwrap();
        assert_eq!(bal.value(), Decimal::new(70_00, 2)); // $70.00
    }

    #[test]
    fn balance_returns_overflow_err_when_debits_exceed_credits_on_unconstrained_account() {
        let mut account = usd_account();
        // Force debits > credits by bypassing the constraint (no flag set)
        account.apply_debit(usd_amount(1_000)).unwrap();
        // credits=0, debits=$10 → balance would be negative → AmountOverflow
        let err = account.balance().unwrap_err();
        assert!(matches!(err, BlazerError::AmountOverflow));
    }

    #[test]
    fn from_str_yields_valid_id_for_account() {
        let id = AccountId::from_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
    }
}

//! Double-entry accounting enforcement layer.
//!
//! This module is the **accounting guardian** for all money movement.
//! Before any transfer touches the ledger, `validate_transfer` ensures:
//!
//! 1. The transfer is not a self-transfer.
//! 2. The transfer amount is positive.
//! 3. Currency matches the debit account.
//! 4. Currency matches the credit account.
//! 5. The debit account has sufficient funds (if constrained).
//!
//! All five rules must pass before a transfer is submitted to TigerBeetle.
//! These checks are intentionally redundant — TigerBeetle also enforces them
//! at the database level — but catching violations in Rust prevents wasteful
//! round-trips and provides cleaner error messages.
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::double_entry::validate_transfer;
//! use blazil_ledger::account::{Account, AccountFlags};
//! use blazil_ledger::transfer::Transfer;
//! use blazil_common::ids::{AccountId, LedgerId, TransferId};
//! use blazil_common::amount::Amount;
//! use blazil_common::currency::parse_currency;
//! use rust_decimal::Decimal;
//!
//! let usd = parse_currency("USD").unwrap();
//! let debit  = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
//! let credit = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
//! let amount = Amount::new(Decimal::new(1_000, 2), usd).unwrap();
//! let transfer = Transfer::new(
//!     TransferId::new(), *debit.id(), *credit.id(), amount, LedgerId::USD, 1,
//! ).unwrap();
//! assert!(validate_transfer(&transfer, &debit, &credit).is_ok());
//! ```

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::traits::Validate;

use crate::account::Account;
use crate::transfer::Transfer;

/// Validates a transfer against its two counterpart accounts.
///
/// This is the full double-entry validation that requires both accounts.
/// It must be called before submitting any transfer to the ledger.
///
/// # Rules (in order)
///
/// 1. `transfer.debit_account_id ≠ transfer.credit_account_id`
/// 2. `transfer.amount > 0`
/// 3. `transfer.amount.currency() == debit_account.currency()`
/// 4. `transfer.amount.currency() == credit_account.currency()`
/// 5. `debit_account.can_debit(&transfer.amount) == true`
///
/// # Errors
///
/// Returns the first rule violation encountered:
///
/// - [`BlazerError::ValidationError`] for rules 1 and 2.
/// - [`BlazerError::CurrencyMismatch`] for rules 3 and 4.
/// - [`BlazerError::InsufficientFunds`] for rule 5.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::double_entry::validate_transfer;
/// use blazil_ledger::account::{Account, AccountFlags};
/// use blazil_ledger::transfer::Transfer;
/// use blazil_common::ids::{AccountId, LedgerId, TransferId};
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let debit  = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
/// let credit = Account::new(AccountId::new(), LedgerId::USD, usd.clone(), 1, AccountFlags::default());
/// let amount = Amount::new(Decimal::new(100, 2), usd).unwrap();
/// let transfer = Transfer::new(
///     TransferId::new(), *debit.id(), *credit.id(), amount, LedgerId::USD, 1,
/// ).unwrap();
/// assert!(validate_transfer(&transfer, &debit, &credit).is_ok());
/// ```
pub fn validate_transfer(
    transfer: &Transfer,
    debit_account: &Account,
    credit_account: &Account,
) -> BlazerResult<()> {
    // Rule 1: no self-transfers
    if transfer.debit_account_id() == transfer.credit_account_id() {
        return Err(BlazerError::ValidationError(
            "self-transfer not allowed".to_owned(),
        ));
    }

    // Rule 2: amount must be positive
    if transfer.amount().value().is_zero() {
        return Err(BlazerError::ValidationError(
            "zero amount transfer".to_owned(),
        ));
    }

    // Rule 3: transfer currency must match the debit account
    if transfer.amount().currency() != debit_account.currency() {
        return Err(BlazerError::CurrencyMismatch {
            expected: debit_account.currency().code().to_owned(),
            actual: transfer.amount().currency().code().to_owned(),
        });
    }

    // Rule 4: transfer currency must match the credit account
    if transfer.amount().currency() != credit_account.currency() {
        return Err(BlazerError::CurrencyMismatch {
            expected: credit_account.currency().code().to_owned(),
            actual: transfer.amount().currency().code().to_owned(),
        });
    }

    // Rule 5: debit account must be able to cover the amount
    if !debit_account.can_debit(transfer.amount()) {
        let available = debit_account
            .balance()
            .map(|b| b.value().to_string())
            .unwrap_or_else(|_| "0".to_owned());
        return Err(BlazerError::InsufficientFunds {
            available,
            required: transfer.amount().value().to_string(),
        });
    }

    Ok(())
}

/// Validates an account's structural invariants.
///
/// Delegates to [`Account::validate`] via the [`Validate`] trait.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::double_entry::validate_account;
/// use blazil_ledger::account::{Account, AccountFlags};
/// use blazil_common::ids::{AccountId, LedgerId};
/// use blazil_common::currency::parse_currency;
///
/// let usd = parse_currency("USD").unwrap();
/// let account = Account::new(AccountId::new(), LedgerId::USD, usd, 1, AccountFlags::default());
/// assert!(validate_account(&account).is_ok());
/// ```
pub fn validate_account(account: &Account) -> BlazerResult<()> {
    account.validate()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::error::BlazerError;
    use blazil_common::ids::{AccountId, LedgerId, TransferId};
    use rust_decimal::Decimal;

    use crate::account::{Account, AccountFlags};
    use crate::transfer::Transfer;

    fn usd_amount(cents: i64) -> Amount {
        Amount::new(Decimal::new(cents, 2), parse_currency("USD").unwrap()).unwrap()
    }

    fn eur_amount(cents: i64) -> Amount {
        Amount::new(Decimal::new(cents, 2), parse_currency("EUR").unwrap()).unwrap()
    }

    fn usd_account() -> Account {
        Account::new(
            AccountId::new(),
            LedgerId::USD,
            parse_currency("USD").unwrap(),
            1,
            AccountFlags::default(),
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

    fn make_transfer(debit: &Account, credit: &Account, amount: Amount) -> Transfer {
        Transfer::new(
            TransferId::new(),
            *debit.id(),
            *credit.id(),
            amount,
            LedgerId::USD,
            1,
        )
        .unwrap()
    }

    #[test]
    fn validate_transfer_passes_with_matching_currencies() {
        let debit = usd_account();
        let credit = usd_account();
        let transfer = make_transfer(&debit, &credit, usd_amount(1_000));
        assert!(validate_transfer(&transfer, &debit, &credit).is_ok());
    }

    #[test]
    fn validate_transfer_fails_on_debit_currency_mismatch() {
        let debit = usd_account();
        let credit = usd_account();
        // EUR amount against USD debit account
        let eur_transfer = Transfer::new(
            TransferId::new(),
            *debit.id(),
            *credit.id(),
            eur_amount(1_000),
            LedgerId::USD,
            1,
        )
        .unwrap();
        let err = validate_transfer(&eur_transfer, &debit, &credit).unwrap_err();
        assert!(matches!(err, BlazerError::CurrencyMismatch { .. }));
    }

    #[test]
    fn validate_transfer_fails_on_credit_currency_mismatch() {
        let debit = usd_account();
        let credit = eur_account();
        let transfer = make_transfer(&debit, &credit, usd_amount(1_000));
        let err = validate_transfer(&transfer, &debit, &credit).unwrap_err();
        assert!(matches!(err, BlazerError::CurrencyMismatch { .. }));
    }

    #[test]
    fn validate_transfer_fails_on_insufficient_funds() {
        let usd = parse_currency("USD").unwrap();
        let flags = AccountFlags {
            debits_must_not_exceed_credits: true,
            ..AccountFlags::default()
        };
        let debit = Account::new(AccountId::new(), LedgerId::USD, usd, 1, flags);
        let credit = usd_account();
        let transfer = make_transfer(&debit, &credit, usd_amount(1_000));
        let err = validate_transfer(&transfer, &debit, &credit).unwrap_err();
        assert!(matches!(err, BlazerError::InsufficientFunds { .. }));
    }

    #[test]
    fn validate_transfer_fails_on_self_transfer() {
        let account = usd_account();
        // Use new_unchecked to bypass Transfer::new self-transfer guard so we
        // can verify that validate_transfer also rejects it independently.
        let t = Transfer::new_unchecked(
            TransferId::new(),
            *account.id(),
            *account.id(),
            usd_amount(1_000),
            LedgerId::USD,
            1,
        );
        let err = validate_transfer(&t, &account, &account).unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn validate_account_delegates_to_account_validate() {
        let account = usd_account();
        assert!(validate_account(&account).is_ok());
    }
}

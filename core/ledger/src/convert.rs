//! Type conversion between Blazil domain types and TigerBeetle wire types.
//!
//! TigerBeetle stores all amounts as raw `u128` integers representing minor
//! units of a currency (e.g. cents for USD, subunits for JPY). This module
//! provides the round-trip conversions that preserve decimal precision.
//!
//! # Amount encoding
//!
//! | Currency | Scale | Minor unit     |
//! |----------|-------|----------------|
//! | USD      | 100   | cents          |
//! | EUR      | 100   | euro-cents     |
//! | GBP      | 100   | pence          |
//! | JPY      | 1     | yen (no sub)   |
//! | VND      | 1     | đồng (no sub)  |
//! | BTC      | 10^8  | satoshis       |
//! | ETH      | 10^18 | wei            |
//! | default  | 100   | generic minor  |
//!
//! # UUID ↔ u128 encoding
//!
//! TigerBeetle identifies accounts and transfers by `u128`. Blazil IDs wrap
//! UUIDs. The mapping is big-endian: a UUID's 16 bytes are treated as a
//! big-endian `u128`.
//!
//! # Examples
//!
//! ```rust
//! use blazil_ledger::convert::{amount_to_minor_units, minor_units_to_amount, account_id_to_u128, u128_to_account_id};
//! use blazil_common::amount::Amount;
//! use blazil_common::currency::parse_currency;
//! use rust_decimal::Decimal;
//!
//! let usd = parse_currency("USD").unwrap();
//! let amount = Amount::new(Decimal::new(100_00, 2), usd).unwrap(); // $100.00
//! let minor = amount_to_minor_units(&amount).unwrap();
//! assert_eq!(minor, 10_000_u128); // 10 000 cents
//! let back = minor_units_to_amount(minor, parse_currency("USD").unwrap()).unwrap();
//! assert_eq!(back.value(), Decimal::new(100_00, 2));
//! ```

use blazil_common::amount::Amount;
use blazil_common::currency::Currency;
use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId, TransferId};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::str::FromStr;

// ── Scale lookup ──────────────────────────────────────────────────────────────

/// Returns the number of minor units per major unit for a given currency.
///
/// Uses a hard-coded lookup table. Currencies not in the table default to 100
/// (two decimal places), which is correct for the vast majority of fiat
/// currencies.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::currency_scale;
/// use blazil_common::currency::parse_currency;
///
/// assert_eq!(currency_scale(&parse_currency("USD").unwrap()), 100);
/// assert_eq!(currency_scale(&parse_currency("JPY").unwrap()), 1);
/// ```
pub fn currency_scale(currency: &Currency) -> u64 {
    match currency.code() {
        "JPY" | "VND" | "KRW" | "CLP" | "ISK" | "PYG" | "UGX" | "XOF" | "GNF" | "IDR" => 1,
        // Crypto: not ISO 4217 — present for future extension when a custom
        // currency type is introduced. Unreachable with the current Currency
        // enum but documented here for clarity.
        "BTC" | "XBT" => 100_000_000,
        "ETH" => 1_000_000_000_000_000_000,
        // Default: two decimal places (USD, EUR, GBP, AUD, CAD, CHF, …)
        _ => 100,
    }
}

// ── Amount conversion ─────────────────────────────────────────────────────────

/// Converts a [`Amount`] to TigerBeetle minor units (`u128`).
///
/// Multiplies `amount.value()` by the currency's scale factor. The result must
/// be an integer (no fractional minor units) and must fit within`u128`.
///
/// # Errors
///
/// Returns [`BlazerError::AmountOverflow`] if the scaled value exceeds `u128`'s
/// range or cannot be represented as an integer.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::amount_to_minor_units;
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let amount = Amount::new(Decimal::new(100_00, 2), usd).unwrap();
/// assert_eq!(amount_to_minor_units(&amount).unwrap(), 10_000);
/// ```
pub fn amount_to_minor_units(amount: &Amount) -> BlazerResult<u128> {
    let scale = currency_scale(amount.currency());
    let scale_decimal = Decimal::from(scale);
    let scaled = amount
        .value()
        .checked_mul(scale_decimal)
        .ok_or(BlazerError::AmountOverflow)?;
    // Reject fractional minor units — e.g. USD 0.001 * 100 = 0.1 is not valid.
    if !scaled.fract().is_zero() {
        return Err(BlazerError::AmountOverflow);
    }
    scaled.to_u128().ok_or(BlazerError::AmountOverflow)
}

/// Reconstructs an [`Amount`] from TigerBeetle minor units and a [`Currency`].
///
/// Divides `minor_units` by the currency's scale factor to recover the decimal
/// value.
///
/// # Errors
///
/// Returns [`BlazerError::AmountOverflow`] if `minor_units` exceeds
/// [`Decimal`]'s representable range (> 28 significant digits), or if the
/// resulting [`Amount`] is invalid.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::minor_units_to_amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let amount = minor_units_to_amount(10_000, parse_currency("USD").unwrap()).unwrap();
/// assert_eq!(amount.value(), Decimal::new(100_00, 2));
/// ```
pub fn minor_units_to_amount(minor_units: u128, currency: Currency) -> BlazerResult<Amount> {
    let scale = currency_scale(&currency);
    let scale_decimal = Decimal::from(scale);
    // Decimal::from_str handles values larger than u64::MAX
    let minor_decimal =
        Decimal::from_str(&minor_units.to_string()).map_err(|_| BlazerError::AmountOverflow)?;
    let value = minor_decimal
        .checked_div(scale_decimal)
        .ok_or(BlazerError::AmountOverflow)?;
    Amount::new(value, currency)
}

// ── ID conversions ────────────────────────────────────────────────────────────

/// Converts an [`AccountId`] to a TigerBeetle `u128` identifier.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::{account_id_to_u128, u128_to_account_id};
/// use blazil_common::ids::AccountId;
///
/// let id = AccountId::new();
/// let raw = account_id_to_u128(&id);
/// assert_eq!(u128_to_account_id(raw), id);
/// ```
pub fn account_id_to_u128(id: &AccountId) -> u128 {
    id.as_u64() as u128
}

/// Reconstructs an [`AccountId`] from a TigerBeetle `u128`.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::{account_id_to_u128, u128_to_account_id};
/// use blazil_common::ids::AccountId;
///
/// let id = AccountId::new();
/// assert_eq!(u128_to_account_id(account_id_to_u128(&id)), id);
/// ```
pub fn u128_to_account_id(raw: u128) -> AccountId {
    AccountId::from_u64(raw as u64)
}

/// Converts a [`TransferId`] to a TigerBeetle `u128` identifier (big-endian).
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::{transfer_id_to_u128, u128_to_transfer_id};
/// use blazil_common::ids::TransferId;
///
/// let id = TransferId::new();
/// assert_eq!(u128_to_transfer_id(transfer_id_to_u128(&id)), id);
/// ```
pub fn transfer_id_to_u128(id: &TransferId) -> u128 {
    u128::from_be_bytes(*id.as_uuid().as_bytes())
}

/// Reconstructs a [`TransferId`] from a TigerBeetle `u128` (big-endian).
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::{transfer_id_to_u128, u128_to_transfer_id};
/// use blazil_common::ids::TransferId;
///
/// let id = TransferId::new();
/// assert_eq!(u128_to_transfer_id(transfer_id_to_u128(&id)), id);
/// ```
pub fn u128_to_transfer_id(raw: u128) -> TransferId {
    TransferId::from_bytes(raw.to_be_bytes())
}

/// Converts a [`LedgerId`] to a TigerBeetle ledger `u32`.
///
/// Since [`LedgerId`] is a `u32` wrapper, this is a direct read of the inner
/// value.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::ledger_id_to_u32;
/// use blazil_common::ids::LedgerId;
///
/// assert_eq!(ledger_id_to_u32(&LedgerId::USD), 1);
/// ```
pub fn ledger_id_to_u32(id: &LedgerId) -> u32 {
    id.value()
}

/// Maps a [`LedgerId`] to its corresponding [`Currency`].
///
/// Used when reconstructing a rich `Amount` from a raw `amount_units: u64` at
/// the ledger handler boundary.
///
/// # Errors
///
/// Returns [`BlazerError::ValidationError`] for any unrecognised ledger id.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::ledger_id_to_currency;
/// use blazil_common::ids::LedgerId;
///
/// let c = ledger_id_to_currency(&LedgerId::USD).unwrap();
/// assert_eq!(c.code(), "USD");
/// ```
pub fn ledger_id_to_currency(id: &LedgerId) -> BlazerResult<Currency> {
    use std::str::FromStr;
    let code = match id.value() {
        1 => "USD",
        2 => "EUR",
        3 => "GBP",
        4 => "JPY",
        5 => "VND",
        6 => "BTC",
        7 => "ETH",
        other => {
            return Err(BlazerError::ValidationError(format!(
                "unknown ledger_id: {other}"
            )))
        }
    };
    Currency::from_str(code)
        .map_err(|_| BlazerError::ValidationError(format!("bad currency code: {code}")))
}

/// Converts a [`TransactionId`] to a TigerBeetle `u128` identifier.
///
/// # Examples
///
/// ```rust
/// use blazil_ledger::convert::transaction_id_to_u128;
/// use blazil_common::ids::TransactionId;
///
/// let id = TransactionId::new();
/// let _ = transaction_id_to_u128(&id);
/// ```
pub fn transaction_id_to_u128(id: &TransactionId) -> u128 {
    id.as_u64() as u128
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use blazil_common::amount::Amount;
    use blazil_common::currency::parse_currency;
    use blazil_common::ids::{AccountId, TransferId};
    use rust_decimal::Decimal;

    // ── amount round-trips ────────────────────────────────────────────────────

    #[test]
    fn usd_100_roundtrip() {
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
        let minor = amount_to_minor_units(&amount).unwrap();
        assert_eq!(minor, 10_000_u128);
        let back = minor_units_to_amount(minor, usd).unwrap();
        assert_eq!(back.value(), amount.value());
    }

    #[test]
    fn vnd_50000_roundtrip() {
        let vnd = parse_currency("VND").unwrap();
        // VND has no sub-units, scale = 1
        let amount = Amount::new(Decimal::new(50_000, 0), vnd).unwrap();
        let minor = amount_to_minor_units(&amount).unwrap();
        assert_eq!(minor, 50_000_u128);
        let back = minor_units_to_amount(minor, vnd).unwrap();
        assert_eq!(back.value(), amount.value());
    }

    #[test]
    fn jpy_1000_roundtrip() {
        let jpy = parse_currency("JPY").unwrap();
        let amount = Amount::new(Decimal::new(1_000, 0), jpy).unwrap();
        let minor = amount_to_minor_units(&amount).unwrap();
        assert_eq!(minor, 1_000_u128);
        let back = minor_units_to_amount(minor, jpy).unwrap();
        assert_eq!(back.value(), Decimal::new(1_000, 0));
    }

    #[test]
    fn eur_99_99_roundtrip() {
        let eur = parse_currency("EUR").unwrap();
        // EUR 99.99 → 9999 euro-cents → EUR 99.99
        let amount = Amount::new(Decimal::new(99_99, 2), eur).unwrap();
        let minor = amount_to_minor_units(&amount).unwrap();
        assert_eq!(minor, 9_999_u128);
        let back = minor_units_to_amount(minor, eur).unwrap();
        assert_eq!(back.value(), amount.value());
    }

    #[test]
    fn amount_overflow_on_non_integer_scaled_result() {
        // USD 0.001 * 100 = 0.1 — fractional minor unit; must be rejected.
        let usd = parse_currency("USD").unwrap();
        let amount = Amount::new(Decimal::new(1, 3), usd).unwrap(); // 0.001
        let err = amount_to_minor_units(&amount).unwrap_err();
        assert!(matches!(err, BlazerError::AmountOverflow));
    }

    // ── UUID round-trips ──────────────────────────────────────────────────────

    #[test]
    fn account_id_u128_roundtrip() {
        let id = AccountId::new();
        let raw = account_id_to_u128(&id);
        assert_eq!(u128_to_account_id(raw), id);
    }

    #[test]
    fn transfer_id_u128_roundtrip() {
        let id = TransferId::new();
        let raw = transfer_id_to_u128(&id);
        assert_eq!(u128_to_transfer_id(raw), id);
    }

    #[test]
    fn u128_zero_yields_zero_account_id() {
        let id = u128_to_account_id(0);
        assert!(id.is_zero());
    }

    #[test]
    fn distinct_account_ids_produce_distinct_u128s() {
        let a = AccountId::new();
        let b = AccountId::new();
        assert_ne!(account_id_to_u128(&a), account_id_to_u128(&b));
    }

    // ── scale lookup ─────────────────────────────────────────────────────────

    #[test]
    fn usd_scale_is_100() {
        assert_eq!(currency_scale(&parse_currency("USD").unwrap()), 100);
    }

    #[test]
    fn jpy_scale_is_1() {
        assert_eq!(currency_scale(&parse_currency("JPY").unwrap()), 1);
    }

    #[test]
    fn vnd_scale_is_1() {
        assert_eq!(currency_scale(&parse_currency("VND").unwrap()), 1);
    }

    #[test]
    fn eur_scale_is_100() {
        assert_eq!(currency_scale(&parse_currency("EUR").unwrap()), 100);
    }
}

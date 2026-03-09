//! ISO 4217 currency code support.
//!
//! Re-exports [`Currency`] from the `iso_currency` crate as the single
//! source of truth for currency codes across the entire Blazil workspace.
//! All other crates import `Currency` from `blazil_common`, never directly
//! from `iso_currency`.
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::currency::{Currency, parse_currency};
//!
//! let usd = parse_currency("USD").unwrap();
//! let eur = parse_currency("EUR").unwrap();
//!
//! assert_ne!(usd, eur);
//! assert!(parse_currency("NOTACURRENCY").is_err());
//! ```

use std::str::FromStr;

pub use iso_currency::Currency;

use crate::error::{BlazerError, BlazerResult};

/// Parses an ISO 4217 alphabetic currency code string into a [`Currency`].
///
/// Accepts standard three-letter ISO 4217 codes (e.g. `"USD"`, `"EUR"`,
/// `"VND"`, `"JPY"`). Codes are case-sensitive; use uppercase as per the
/// ISO 4217 standard.
///
/// # Errors
///
/// Returns [`BlazerError::InvalidCurrency`] if `code` is not a recognised
/// ISO 4217 alphabetic code.
///
/// # Examples
///
/// ```rust
/// use blazil_common::currency::parse_currency;
///
/// assert!(parse_currency("USD").is_ok());
/// assert!(parse_currency("EUR").is_ok());
/// assert!(parse_currency("VND").is_ok());
/// assert!(parse_currency("INVALID").is_err());
/// ```
pub fn parse_currency(code: &str) -> BlazerResult<Currency> {
    Currency::from_str(code)
        .map_err(|_| BlazerError::InvalidCurrency(code.to_owned()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usd_succeeds() {
        let c = parse_currency("USD").unwrap();
        assert_eq!(c, Currency::USD);
    }

    #[test]
    fn parse_eur_succeeds() {
        let c = parse_currency("EUR").unwrap();
        assert_eq!(c, Currency::EUR);
    }

    #[test]
    fn parse_vnd_succeeds() {
        // Vietnamese Dong — commonly used in Southeast Asian fintech
        let c = parse_currency("VND").unwrap();
        assert_eq!(c, Currency::VND);
    }

    #[test]
    fn parse_invalid_code_returns_error() {
        let err = parse_currency("INVALID").unwrap_err();
        assert!(
            matches!(err, BlazerError::InvalidCurrency(_)),
            "expected InvalidCurrency, got: {:?}",
            err
        );
    }

    #[test]
    fn parse_empty_string_returns_error() {
        let err = parse_currency("").unwrap_err();
        assert!(matches!(err, BlazerError::InvalidCurrency(_)));
    }

    #[test]
    fn parse_lowercase_returns_error() {
        // ISO 4217 codes are uppercase; lowercase must be rejected
        let err = parse_currency("usd").unwrap_err();
        assert!(matches!(err, BlazerError::InvalidCurrency(_)));
    }

    #[test]
    fn different_currencies_are_not_equal() {
        let usd = parse_currency("USD").unwrap();
        let eur = parse_currency("EUR").unwrap();
        assert_ne!(usd, eur);
    }

    #[test]
    fn same_currency_parses_equal() {
        let a = parse_currency("USD").unwrap();
        let b = parse_currency("USD").unwrap();
        assert_eq!(a, b);
    }
}

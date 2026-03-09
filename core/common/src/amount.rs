//! Fixed-point monetary amounts with currency-aware arithmetic.
//!
//! [`Amount`] couples a [`rust_decimal::Decimal`] value with an ISO 4217
//! [`Currency`]. All arithmetic is currency-checked and overflow-safe.
//!
//! # No floating point — ever
//!
//! `f32` and `f64` are **forbidden** in all monetary calculations. A float
//! balance is a bug waiting to destroy someone's money. `rust_decimal`
//! provides exact decimal arithmetic at the precision required by financial
//! regulations (up to 8 decimal places).
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::amount::Amount;
//! use blazil_common::currency::parse_currency;
//! use rust_decimal::Decimal;
//!
//! let usd = parse_currency("USD").unwrap();
//! let a = Amount::new(Decimal::new(10_000, 2), usd).unwrap(); // 100.00 USD
//! let b = Amount::new(Decimal::new(5_000, 2), usd).unwrap();  //  50.00 USD
//!
//! let sum = a.checked_add(b).unwrap();
//! assert_eq!(sum.to_string(), "150.00 USD");
//! ```

use std::fmt;

use rust_decimal::Decimal;

use crate::currency::Currency;
use crate::error::{BlazerError, BlazerResult};

/// Maximum permitted scale (decimal places) for a monetary amount.
const MAX_SCALE: u32 = 8;

/// A non-negative monetary amount paired with its ISO 4217 currency.
///
/// # Invariants
///
/// The following invariants are enforced at construction time and cannot
/// be violated by safe code:
///
/// - `value >= 0` — amounts are non-negative; directionality is in the transaction.
/// - `value.scale() <= 8` — at most 8 decimal places.
/// - `value < Decimal::MAX / 2` — overflow guard for subsequent arithmetic.
///
/// # Examples
///
/// ```rust
/// use blazil_common::amount::Amount;
/// use blazil_common::currency::parse_currency;
/// use rust_decimal::Decimal;
///
/// let usd = parse_currency("USD").unwrap();
/// let amount = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
/// assert_eq!(amount.to_string(), "100.00 USD");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    value: Decimal,
    currency: Currency,
}

impl Amount {
    /// Constructs a new [`Amount`], enforcing all monetary invariants.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::NegativeAmount`] — if `value < 0`.
    /// - [`BlazerError::InvalidAmountScale`] — if `value.scale() > 8`.
    /// - [`BlazerError::AmountOverflow`] — if `value >= Decimal::MAX / 2`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let ok = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
    /// assert_eq!(ok.to_string(), "100.00 USD");
    ///
    /// let neg = Amount::new(Decimal::new(-1, 0), usd);
    /// assert!(neg.is_err());
    /// ```
    pub fn new(value: Decimal, currency: Currency) -> BlazerResult<Self> {
        if value < Decimal::ZERO {
            return Err(BlazerError::NegativeAmount);
        }
        if value.scale() > MAX_SCALE {
            return Err(BlazerError::InvalidAmountScale(value.scale()));
        }
        // Overflow protection: ensure we have headroom for addition.
        // Decimal::MAX / 2 computed at runtime — safe since 2 is non-zero.
        if value >= Decimal::MAX / Decimal::TWO {
            return Err(BlazerError::AmountOverflow);
        }
        Ok(Self { value, currency })
    }

    /// Returns an [`Amount`] of exactly zero in the given currency.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let zero = Amount::zero(usd);
    /// assert_eq!(zero.value(), Decimal::ZERO);
    /// ```
    #[must_use]
    pub fn zero(currency: Currency) -> Self {
        // SAFETY: Decimal::ZERO has scale 0, is non-negative, and well within limits.
        Self {
            value: Decimal::ZERO,
            currency,
        }
    }

    /// Returns the decimal value of this amount (read-only).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let a = Amount::new(Decimal::new(5_000, 2), usd).unwrap();
    /// assert_eq!(a.value(), Decimal::new(5_000, 2));
    /// ```
    #[must_use]
    pub fn value(&self) -> Decimal {
        self.value
    }

    /// Returns a reference to the ISO 4217 currency of this amount.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::{Currency, parse_currency};
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let a = Amount::new(Decimal::new(100, 0), usd).unwrap();
    /// assert_eq!(*a.currency(), Currency::USD);
    /// ```
    #[must_use]
    pub fn currency(&self) -> &Currency {
        &self.currency
    }

    /// Adds `other` to `self`, returning the sum as a new [`Amount`].
    ///
    /// # Errors
    ///
    /// - [`BlazerError::CurrencyMismatch`] — if the currencies differ.
    /// - [`BlazerError::AmountOverflow`] — if the result exceeds the maximum.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let a = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
    /// let b = Amount::new(Decimal::new(5_000, 2), usd).unwrap();
    /// let sum = a.checked_add(b).unwrap();
    /// assert_eq!(sum.to_string(), "150.00 USD");
    /// ```
    pub fn checked_add(self, other: Amount) -> BlazerResult<Amount> {
        if self.currency != other.currency {
            return Err(BlazerError::CurrencyMismatch {
                expected: self.currency.code().to_owned(),
                actual: other.currency.code().to_owned(),
            });
        }
        let result_value = self
            .value
            .checked_add(other.value)
            .ok_or(BlazerError::AmountOverflow)?;
        Ok(Amount {
            value: result_value,
            currency: self.currency,
        })
    }

    /// Subtracts `other` from `self`, returning the difference as a new [`Amount`].
    ///
    /// # Errors
    ///
    /// - [`BlazerError::CurrencyMismatch`] — if the currencies differ.
    /// - [`BlazerError::InsufficientFunds`] — if `other > self`.
    /// - [`BlazerError::AmountOverflow`] — on internal arithmetic overflow (extremely unlikely).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::amount::Amount;
    /// use blazil_common::currency::parse_currency;
    /// use rust_decimal::Decimal;
    ///
    /// let usd = parse_currency("USD").unwrap();
    /// let a = Amount::new(Decimal::new(10_000, 2), usd).unwrap();
    /// let b = Amount::new(Decimal::new(3_000, 2), usd).unwrap();
    /// let diff = a.checked_sub(b).unwrap();
    /// assert_eq!(diff.to_string(), "70.00 USD");
    /// ```
    pub fn checked_sub(self, other: Amount) -> BlazerResult<Amount> {
        if self.currency != other.currency {
            return Err(BlazerError::CurrencyMismatch {
                expected: self.currency.code().to_owned(),
                actual: other.currency.code().to_owned(),
            });
        }
        if other.value > self.value {
            return Err(BlazerError::InsufficientFunds {
                available: self.value.to_string(),
                required: other.value.to_string(),
            });
        }
        let result_value = self
            .value
            .checked_sub(other.value)
            .ok_or(BlazerError::AmountOverflow)?;
        Ok(Amount {
            value: result_value,
            currency: self.currency,
        })
    }
}

impl fmt::Display for Amount {
    /// Formats as `"<value> <currency-code>"`, e.g. `"100.00 USD"`.
    ///
    /// The decimal value is rendered with its natural scale (no rounding).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.value, self.currency.code())
    }
}

// ── Manual Serde implementation ───────────────────────────────────────────────
//
// We implement Serialize/Deserialize manually so that:
//   - `value` serializes as a string (preserving exact precision).
//   - `currency` serializes as the 3-letter ISO code string.
// This avoids any dependency on iso_currency's optional serde feature.

impl serde::Serialize for Amount {
    /// Serializes as `{"value": "100.00", "currency": "USD"}`.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Amount", 2)?;
        state.serialize_field("value", &self.value.to_string())?;
        state.serialize_field("currency", &self.currency.code())?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Amount {
    /// Deserializes from `{"value": "<decimal-string>", "currency": "<ISO-code>"}`.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use std::str::FromStr;

        // Helper struct to capture the raw JSON fields
        #[derive(serde::Deserialize)]
        struct AmountHelper {
            value: String,
            currency: String,
        }

        let helper = AmountHelper::deserialize(deserializer)?;

        let value = Decimal::from_str(&helper.value).map_err(serde::de::Error::custom)?;

        let currency =
            crate::currency::parse_currency(&helper.currency).map_err(serde::de::Error::custom)?;

        Amount::new(value, currency).map_err(serde::de::Error::custom)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::currency::parse_currency;

    fn usd() -> Currency {
        parse_currency("USD").unwrap()
    }

    fn eur() -> Currency {
        parse_currency("EUR").unwrap()
    }

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn new_with_valid_value_succeeds() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        assert_eq!(a.value(), Decimal::new(10_000, 2));
    }

    #[test]
    fn new_with_negative_value_returns_negative_amount_error() {
        let err = Amount::new(Decimal::new(-1, 0), usd()).unwrap_err();
        assert!(
            matches!(err, BlazerError::NegativeAmount),
            "expected NegativeAmount, got: {:?}",
            err
        );
    }

    #[test]
    fn new_with_scale_greater_than_8_returns_scale_error() {
        // scale 9 = 9 decimal places → exceeds MAX_SCALE
        let err = Amount::new(Decimal::new(1, 9), usd()).unwrap_err();
        assert!(
            matches!(err, BlazerError::InvalidAmountScale(9)),
            "expected InvalidAmountScale(9), got: {:?}",
            err
        );
    }

    #[test]
    fn new_with_scale_exactly_8_succeeds() {
        // 8 decimal places is the maximum allowed
        let a = Amount::new(Decimal::new(1, 8), usd()).unwrap();
        assert_eq!(a.value().scale(), 8);
    }

    #[test]
    fn zero_returns_decimal_zero() {
        let z = Amount::zero(usd());
        assert_eq!(z.value(), Decimal::ZERO);
        assert_eq!(*z.currency(), usd());
    }

    // ── checked_add ──────────────────────────────────────────────────────────

    #[test]
    fn checked_add_same_currency_returns_correct_result() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap(); // 100.00
        let b = Amount::new(Decimal::new(5_000, 2), usd()).unwrap(); //  50.00
        let sum = a.checked_add(b).unwrap();
        assert_eq!(sum.value(), Decimal::new(15_000, 2)); // 150.00
        assert_eq!(*sum.currency(), usd());
    }

    #[test]
    fn checked_add_different_currency_returns_mismatch_error() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let b = Amount::new(Decimal::new(5_000, 2), eur()).unwrap();
        let err = a.checked_add(b).unwrap_err();
        assert!(
            matches!(err, BlazerError::CurrencyMismatch { .. }),
            "expected CurrencyMismatch, got: {:?}",
            err
        );
    }

    // ── checked_sub ──────────────────────────────────────────────────────────

    #[test]
    fn checked_sub_with_sufficient_funds_returns_correct_result() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap(); // 100.00
        let b = Amount::new(Decimal::new(3_000, 2), usd()).unwrap(); //  30.00
        let diff = a.checked_sub(b).unwrap();
        assert_eq!(diff.value(), Decimal::new(7_000, 2)); // 70.00
    }

    #[test]
    fn checked_sub_exact_balance_returns_zero() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let b = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let diff = a.checked_sub(b).unwrap();
        assert_eq!(diff.value(), Decimal::ZERO);
    }

    #[test]
    fn checked_sub_with_insufficient_funds_returns_error() {
        let a = Amount::new(Decimal::new(5_000, 2), usd()).unwrap(); //  50.00
        let b = Amount::new(Decimal::new(10_000, 2), usd()).unwrap(); // 100.00
        let err = a.checked_sub(b).unwrap_err();
        assert!(
            matches!(err, BlazerError::InsufficientFunds { .. }),
            "expected InsufficientFunds, got: {:?}",
            err
        );
    }

    #[test]
    fn checked_sub_different_currency_returns_mismatch_error() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let b = Amount::new(Decimal::new(5_000, 2), eur()).unwrap();
        let err = a.checked_sub(b).unwrap_err();
        assert!(
            matches!(err, BlazerError::CurrencyMismatch { .. }),
            "expected CurrencyMismatch, got: {:?}",
            err
        );
    }

    // ── Display ──────────────────────────────────────────────────────────────

    #[test]
    fn display_formats_as_value_space_currency() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        assert_eq!(a.to_string(), "100.00 USD");
    }

    #[test]
    fn display_zero_amount() {
        let z = Amount::zero(usd());
        assert_eq!(z.to_string(), "0 USD");
    }

    // ── Serde ────────────────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip_preserves_precision() {
        // 1.23456789 — uses full 8 decimal places
        let a = Amount::new(Decimal::new(123_456_789, 8), usd()).unwrap();
        let json = serde_json::to_string(&a).unwrap();

        // value must be a string (not a float) to preserve precision
        assert!(
            json.contains("\"123456789E-8\"") || json.contains("\"1.23456789\""),
            "value should be a decimal string: {json}"
        );

        let deserialized: Amount = serde_json::from_str(&json).unwrap();

        // Mathematical equality — both represent 1.23456789
        assert_eq!(
            a.value().normalize(),
            deserialized.value().normalize(),
            "Decimal values differ after serde round-trip"
        );
        assert_eq!(a.currency(), deserialized.currency());
    }

    #[test]
    fn serde_currency_serialized_as_code_string() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            json.contains("\"USD\""),
            "currency not serialized as string: {json}"
        );
    }

    #[test]
    fn serde_value_serialized_as_string_not_float() {
        let a = Amount::new(Decimal::new(10_000, 2), usd()).unwrap();
        let json = serde_json::to_string(&a).unwrap();
        // The value field must be a JSON string, not a JSON number
        assert!(
            json.contains("\"100.00\""),
            "value should be \"100.00\": {json}"
        );
    }
}

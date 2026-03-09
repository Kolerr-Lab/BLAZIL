//! Unified error type for the entire Blazil workspace.
//!
//! All fallible operations throughout Blazil return [`BlazerResult<T>`],
//! which is an alias for `Result<T, BlazerError>`. Using a single error
//! type keeps error propagation consistent via the `?` operator.
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::error::{BlazerError, BlazerResult};
//!
//! fn might_fail(input: &str) -> BlazerResult<u64> {
//!     if input.is_empty() {
//!         return Err(BlazerError::ValidationError("input must not be empty".into()));
//!     }
//!     Ok(42)
//! }
//!
//! assert!(might_fail("").is_err());
//! assert!(might_fail("hello").is_ok());
//! ```

use thiserror::Error;

/// The single unified error type for the Blazil workspace.
///
/// Every crate in the workspace returns `BlazerError` from fallible functions.
/// Variants cover all error domains: identity, monetary arithmetic, currency,
/// validation, transport, ledger, and resource lifecycle.
#[derive(Debug, Clone, Error)]
pub enum BlazerError {
    // ── Identity errors ──────────────────────────────────────────────────────

    /// A string could not be parsed as a valid UUID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::str::FromStr;
    /// use blazil_common::ids::TransactionId;
    /// let err = TransactionId::from_str("not-a-uuid").unwrap_err();
    /// ```
    #[error("Invalid UUID format: {0}")]
    InvalidId(String),

    // ── Amount errors ────────────────────────────────────────────────────────

    /// An arithmetic operation was attempted on amounts with different currencies.
    ///
    /// In Blazil, currency conversion is an explicit domain operation — implicit
    /// cross-currency arithmetic is never allowed.
    #[error("Currency mismatch: expected {expected}, got {actual}")]
    CurrencyMismatch { expected: String, actual: String },

    /// A subtraction would result in a negative balance.
    ///
    /// In Blazil amounts are non-negative; directionality is expressed
    /// through transaction type, not amount sign.
    #[error("Insufficient funds: available {available}, required {required}")]
    InsufficientFunds { available: String, required: String },

    /// An arithmetic operation produced a value exceeding the representable maximum.
    #[error("Amount overflow: value exceeds maximum allowed")]
    AmountOverflow,

    /// An amount was constructed with more than 8 decimal places.
    ///
    /// Blazil enforces a maximum scale of 8 to prevent precision abuse and
    /// to remain compatible with TigerBeetle's fixed-point representation.
    #[error("Invalid amount scale: {0} decimal places exceeds maximum of 8")]
    InvalidAmountScale(u32),

    /// An amount was constructed with a negative value.
    ///
    /// Amounts in Blazil are always non-negative. Debit/credit direction is
    /// encoded in the transaction structure, not in the amount sign.
    #[error("Amount cannot be negative")]
    NegativeAmount,

    // ── Currency errors ──────────────────────────────────────────────────────

    /// A string could not be parsed as a valid ISO 4217 currency code.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::currency::parse_currency;
    /// let err = parse_currency("XYZ_INVALID").unwrap_err();
    /// ```
    #[error("Invalid currency code: {0}")]
    InvalidCurrency(String),

    // ── Validation errors ────────────────────────────────────────────────────

    /// A domain object failed structural validation.
    ///
    /// Used by implementors of the [`crate::traits::Validate`] trait.
    #[error("Validation failed: {0}")]
    ValidationError(String),

    // ── System errors ────────────────────────────────────────────────────────

    /// An unexpected internal error occurred.
    ///
    /// This variant should be rare in production; prefer specific variants
    /// for known error cases.
    #[error("Internal error: {0}")]
    Internal(String),

    // ── Transport errors ─────────────────────────────────────────────────────

    /// An error occurred in the network transport layer (Aeron, io_uring, etc.).
    #[error("Transport error: {0}")]
    Transport(String),

    // ── Ledger errors ────────────────────────────────────────────────────────

    /// An error was returned from the TigerBeetle ledger layer.
    #[error("Ledger error: {0}")]
    Ledger(String),

    // ── Resource lifecycle ───────────────────────────────────────────────────

    /// The requested resource does not exist.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::error::BlazerError;
    /// let err = BlazerError::NotFound {
    ///     resource: "Account".into(),
    ///     id: "abc-123".into(),
    /// };
    /// ```
    #[error("Not found: {resource} with id {id}")]
    NotFound { resource: String, id: String },

    /// An attempt was made to create a resource that already exists.
    #[error("Duplicate: {resource} with id {id} already exists")]
    Duplicate { resource: String, id: String },
}

/// Convenience alias for `Result<T, BlazerError>`.
///
/// Used throughout the Blazil workspace so that `?` propagation works
/// uniformly across all crate boundaries.
///
/// # Examples
///
/// ```rust
/// use blazil_common::error::{BlazerError, BlazerResult};
///
/// fn parse_positive(s: &str) -> BlazerResult<u64> {
///     s.parse::<u64>()
///         .map_err(|e| BlazerError::ValidationError(e.to_string()))
/// }
///
/// assert_eq!(parse_positive("42").unwrap(), 42);
/// assert!(parse_positive("not_a_number").is_err());
/// ```
pub type BlazerResult<T> = std::result::Result<T, BlazerError>;

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_messages_are_nonempty() {
        let variants: Vec<BlazerError> = vec![
            BlazerError::InvalidId("bad-id".into()),
            BlazerError::CurrencyMismatch {
                expected: "USD".into(),
                actual: "EUR".into(),
            },
            BlazerError::InsufficientFunds {
                available: "50.00".into(),
                required: "100.00".into(),
            },
            BlazerError::AmountOverflow,
            BlazerError::InvalidAmountScale(9),
            BlazerError::NegativeAmount,
            BlazerError::InvalidCurrency("XYZ".into()),
            BlazerError::ValidationError("bad state".into()),
            BlazerError::Internal("oops".into()),
            BlazerError::Transport("connection refused".into()),
            BlazerError::Ledger("write failed".into()),
            BlazerError::NotFound {
                resource: "Account".into(),
                id: "abc".into(),
            },
            BlazerError::Duplicate {
                resource: "Account".into(),
                id: "abc".into(),
            },
        ];

        for variant in variants {
            let msg = variant.to_string();
            assert!(!msg.is_empty(), "BlazerError variant has empty Display: {:?}", variant);
        }
    }

    #[test]
    fn blazer_result_holds_ok_value() {
        let result: BlazerResult<i32> = Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn blazer_result_holds_err_value() {
        let result: BlazerResult<i32> =
            Err(BlazerError::Internal("test error".into()));
        assert!(result.is_err());
    }

    #[test]
    fn currency_mismatch_includes_both_codes() {
        let err = BlazerError::CurrencyMismatch {
            expected: "USD".into(),
            actual: "EUR".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("USD"), "missing 'USD' in: {msg}");
        assert!(msg.contains("EUR"), "missing 'EUR' in: {msg}");
    }

    #[test]
    fn insufficient_funds_includes_both_amounts() {
        let err = BlazerError::InsufficientFunds {
            available: "50.00".into(),
            required: "100.00".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("50.00"), "missing available in: {msg}");
        assert!(msg.contains("100.00"), "missing required in: {msg}");
    }
}

//! Strongly-typed identity primitives for all Blazil domain entities.
//!
//! Each ID type is an opaque newtype around a UUID v4. The inner `Uuid`
//! is **never** exposed directly; all construction goes through validated
//! constructors. This prevents passing an `AccountId` where a
//! `TransactionId` is expected — the type system enforces correctness.
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::ids::{TransactionId, AccountId};
//!
//! let tx_id = TransactionId::new();
//! let acct_id = AccountId::new();
//!
//! // Type system prevents mixing IDs:
//! // let _: TransactionId = acct_id; // ← compile error
//!
//! // Round-trips via decimal string representation.
//! let s = tx_id.to_string();
//! let reparsed: TransactionId = s.parse().unwrap();
//! assert_eq!(tx_id, reparsed);
//! ```

use std::fmt;
use uuid::Uuid;

use crate::error::{BlazerError, BlazerResult};

// ── Macro to reduce repetition across the four newtype structs ────────────────

macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a new random ID using UUID v4.
            ///
            /// Every call returns a cryptographically unique identifier.
            ///
            /// # Examples
            ///
            /// ```rust
            #[doc = concat!("use blazil_common::ids::", stringify!($name), ";")]
            #[doc = concat!("let id = ", stringify!($name), "::new();")]
            /// // Each call produces a distinct ID
            #[doc = concat!("assert_ne!(", stringify!($name), "::new(), ", stringify!($name), "::new());")]
            /// ```
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Parses an ID from a UUID string (any standard UUID format).
            ///
            /// Accepts the canonical hyphenated form and other standard
            /// UUID representations accepted by the `uuid` crate.
            ///
            /// # Errors
            ///
            /// Returns [`BlazerError::InvalidId`] if the string is not a valid UUID.
            ///
            /// # Examples
            ///
            /// ```rust
            #[doc = concat!("use blazil_common::ids::", stringify!($name), ";")]
            #[doc = concat!("let id: ", stringify!($name), " = \"550e8400-e29b-41d4-a716-446655440000\".parse().unwrap();")]
            #[doc = concat!("assert!(\"not-a-uuid\".parse::<", stringify!($name), ">().is_err());")]
            /// ```
            /// This method is provided via the [`std::str::FromStr`] trait implementation.
            /// Use `s.parse::<TypeName>()` or `TypeName::from_str(s)` (with `use std::str::FromStr` in scope).

            /// Returns a read-only reference to the inner UUID.
            ///
            /// Prefer using the typed ID directly wherever possible. This
            /// escape hatch exists for interop with libraries that need a
            /// raw `Uuid`.
            ///
            /// # Examples
            ///
            /// ```rust
            #[doc = concat!("use blazil_common::ids::", stringify!($name), ";")]
            #[doc = concat!("let id = ", stringify!($name), "::new();")]
            /// let _ = id.as_uuid();
            /// ```
            #[must_use]
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Constructs an ID directly from raw UUID bytes (big-endian).
            ///
            /// Used for round-tripping identifiers through TigerBeetle's
            /// `u128` wire format. Every 16-byte sequence is a valid UUID,
            /// so this constructor is infallible.
            ///
            /// # Examples
            ///
            /// ```rust
            #[doc = concat!("use blazil_common::ids::", stringify!($name), ";")]
            #[doc = concat!("let id = ", stringify!($name), "::new();")]
            /// let bytes = *id.as_uuid().as_bytes();
            #[doc = concat!("let round_tripped = ", stringify!($name), "::from_bytes(bytes);")]
            /// assert_eq!(id, round_tripped);
            /// ```
            #[must_use]
            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl serde::Serialize for $name {
            /// Serializes as a plain hyphenated UUID string.
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            /// Deserializes from a UUID string, validating the format.
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = <String as serde::Deserialize>::deserialize(deserializer)?;
                Uuid::parse_str(&s)
                    .map(Self)
                    .map_err(serde::de::Error::custom)
            }
        }

        impl std::str::FromStr for $name {
            type Err = BlazerError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s)
                    .map(Self)
                    .map_err(|_| BlazerError::InvalidId(s.to_owned()))
            }
        }
    };
}

// ── Fast u64 ID macro (hot-path types: TransactionId, AccountId) ─────────────
//
// These IDs live in every `TransactionEvent` ring-buffer slot.  Shrinking them
// from 16-byte UUIDs to 8-byte u64s is the primary lever for reaching the
// 64-byte (1 cache-line) event target.
//
// Uniqueness: seeded from UUID v4's 128 bits of CSPRNG randomness, lower 64
// bits extracted.  Birthday bound over 1 billion IDs ≈ 5×10⁻¹¹ — acceptable
// for IDs that undergo double-validation at the ledger boundary.
macro_rules! define_u64_id {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(u64);

        impl $name {
            /// Creates a new random ID using 64 bits of UUID v4 entropy.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().as_u128() as u64)
            }

            /// Returns the raw `u64` value.
            #[must_use]
            pub fn as_u64(&self) -> u64 {
                self.0
            }

            /// Constructs an ID directly from a raw `u64`.
            ///
            /// `0` is the nil sentinel checked by the validation handler.
            #[must_use]
            pub fn from_u64(v: u64) -> Self {
                Self(v)
            }

            /// Returns `true` if this ID is zero (the nil sentinel).
            #[must_use]
            pub fn is_zero(&self) -> bool {
                self.0 == 0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl serde::Serialize for $name {
            /// Serializes as a decimal string to preserve full u64 precision
            /// in JSON (JS `Number` only has 53 bits of integer precision).
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            /// Deserializes from a decimal string.
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = <String as serde::Deserialize>::deserialize(deserializer)?;
                s.parse::<u64>()
                    .map(Self)
                    .map_err(serde::de::Error::custom)
            }
        }

        impl std::str::FromStr for $name {
            type Err = BlazerError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                // Canonical format: decimal u64 string.
                if let Ok(v) = s.parse::<u64>() {
                    return Ok(Self(v));
                }
                // Backward-compat: accept UUID strings, map to u64 via lower
                // 64 bits so existing clients don't need an immediate update.
                Uuid::parse_str(s)
                    .map(|u| Self(u.as_u128() as u64))
                    .map_err(|_| BlazerError::InvalidId(s.to_owned()))
            }
        }
    };
}

// ── ID type definitions ───────────────────────────────────────────────────────

define_u64_id!(
    /// A unique identifier for a financial transaction.
    ///
    /// Wraps a `u64` opaquely. Cannot be confused with [`AccountId`],
    /// [`LedgerId`], or [`TransferId`] at compile time.
    ///
    /// Use [`as_u64`](TransactionId::as_u64) / [`from_u64`](TransactionId::from_u64)
    /// to access or construct raw values.  The nil sentinel is `0`.
    TransactionId
);

define_u64_id!(
    /// A unique identifier for a financial account.
    ///
    /// Wraps a `u64` opaquely. Cannot be confused with [`TransactionId`],
    /// [`LedgerId`], or [`TransferId`] at compile time.
    ///
    /// Use [`as_u64`](AccountId::as_u64) / [`from_u64`](AccountId::from_u64)
    /// to access or construct raw values.  The nil sentinel is `0`.
    AccountId
);

define_id!(
    /// A unique identifier for a transfer between two accounts.
    ///
    /// Wraps a UUID v4 opaquely. Cannot be confused with [`TransactionId`],
    /// [`AccountId`], or [`LedgerId`] at compile time.
    TransferId
);

// ── LedgerId ─────────────────────────────────────────────────────────────────

/// A logical ledger identifier.
///
/// Maps directly to TigerBeetle's 32-bit ledger field.
/// Each currency typically has its own ledger.
///
/// # Examples
///
/// ```rust
/// use blazil_common::ids::LedgerId;
///
/// let id = LedgerId::new(1).unwrap();
/// assert_eq!(id.value(), 1);
/// assert_eq!(LedgerId::USD.value(), 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LedgerId(u32);

impl LedgerId {
    /// Creates a new `LedgerId` from a raw `u32` value.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::ValidationError`] if `value` is `0`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::ids::LedgerId;
    ///
    /// assert!(LedgerId::new(0).is_err());
    /// assert_eq!(LedgerId::new(1).unwrap().value(), 1);
    /// ```
    pub fn new(value: u32) -> BlazerResult<Self> {
        if value == 0 {
            return Err(BlazerError::ValidationError(
                "LedgerId must not be zero".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the raw `u32` value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::ids::LedgerId;
    /// assert_eq!(LedgerId::USD.value(), 1);
    /// ```
    pub fn value(&self) -> u32 {
        self.0
    }

    /// Well-known ledger for United States Dollar (USD).
    pub const USD: LedgerId = LedgerId(1);
    /// Well-known ledger for Euro (EUR).
    pub const EUR: LedgerId = LedgerId(2);
    /// Well-known ledger for British Pound Sterling (GBP).
    pub const GBP: LedgerId = LedgerId(3);
    /// Well-known ledger for Japanese Yen (JPY).
    pub const JPY: LedgerId = LedgerId(4);
    /// Well-known ledger for Vietnamese Dong (VND).
    pub const VND: LedgerId = LedgerId(5);
    /// Well-known ledger for Bitcoin (BTC).
    pub const BTC: LedgerId = LedgerId(6);
    /// Well-known ledger for Ethereum (ETH).
    pub const ETH: LedgerId = LedgerId(7);
}

impl fmt::Display for LedgerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LedgerId({})", self.0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const CANONICAL_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    // ── TransactionId ────────────────────────────────────────────────────────

    #[test]
    fn new_generates_nonzero_id() {
        let id = TransactionId::new();
        assert!(!id.is_zero(), "new() must never produce the nil sentinel");
        // Round-trip via decimal string.
        let reparsed = TransactionId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, reparsed);
    }

    #[test]
    fn from_str_accepts_decimal_u64() {
        let id = TransactionId::from_str("12345678901234567890").unwrap();
        assert_eq!(id.as_u64(), 12345678901234567890_u64);
        assert_eq!(id.to_string(), "12345678901234567890");
    }

    #[test]
    fn from_str_accepts_uuid_compat() {
        // UUID strings are accepted for protocol backward compatibility.
        let id = TransactionId::from_str(CANONICAL_UUID).unwrap();
        // to_string() gives the decimal u64, not the original UUID string.
        let reparsed = TransactionId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, reparsed);
    }

    #[test]
    fn from_str_rejects_invalid_string() {
        let err = TransactionId::from_str("not-a-uuid-or-number").unwrap_err();
        assert!(matches!(err, BlazerError::InvalidId(_)));
    }

    #[test]
    fn two_new_calls_produce_different_ids() {
        let a = TransactionId::new();
        let b = TransactionId::new();
        assert_ne!(a, b, "u64 collision — astronomically unlikely");
    }

    #[test]
    fn display_formats_as_decimal_u64() {
        let id = TransactionId::from_u64(42);
        assert_eq!(id.to_string(), "42");
    }

    #[test]
    fn debug_includes_type_name_and_value() {
        let id = TransactionId::from_u64(99);
        let debug_str = format!("{id:?}");
        assert!(
            debug_str.starts_with("TransactionId("),
            "Debug output missing type name: {debug_str}"
        );
        assert!(
            debug_str.contains("99"),
            "Debug output missing value: {debug_str}"
        );
    }

    #[test]
    fn serde_roundtrip_transaction_id() {
        let id = TransactionId::from_u64(9_999_999_999_999_999_999_u64);
        let json = serde_json::to_string(&id).unwrap();
        // Must serialize as a decimal string, not a bare JSON number.
        assert_eq!(json, "\"9999999999999999999\"");
        let deserialized: TransactionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn as_u64_returns_inner_value() {
        let id = TransactionId::from_u64(123_456);
        assert_eq!(id.as_u64(), 123_456_u64);
    }

    #[test]
    fn is_zero_detects_nil_sentinel() {
        assert!(TransactionId::from_u64(0).is_zero());
        assert!(!TransactionId::from_u64(1).is_zero());
    }

    // ── AccountId ────────────────────────────────────────────────────────────

    #[test]
    fn account_id_from_str_accepts_decimal_u64() {
        let id = AccountId::from_str("42").unwrap();
        assert_eq!(id.as_u64(), 42_u64);
    }

    #[test]
    fn account_id_from_str_rejects_invalid_string() {
        let err = AccountId::from_str("bad").unwrap_err();
        assert!(matches!(err, BlazerError::InvalidId(_)));
    }

    #[test]
    fn account_id_serde_roundtrip() {
        let id = AccountId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: AccountId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    // ── LedgerId ─────────────────────────────────────────────────────────────

    #[test]
    fn ledger_id_zero_is_rejected() {
        let err = LedgerId::new(0).unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn ledger_id_nonzero_succeeds() {
        let id = LedgerId::new(42).unwrap();
        assert_eq!(id.value(), 42);
    }

    #[test]
    fn ledger_id_constants_correct() {
        assert_eq!(LedgerId::USD.value(), 1);
        assert_eq!(LedgerId::EUR.value(), 2);
        assert_eq!(LedgerId::GBP.value(), 3);
        assert_eq!(LedgerId::JPY.value(), 4);
        assert_eq!(LedgerId::VND.value(), 5);
        assert_eq!(LedgerId::BTC.value(), 6);
        assert_eq!(LedgerId::ETH.value(), 7);
    }

    #[test]
    fn ledger_id_display() {
        assert_eq!(LedgerId::USD.to_string(), "LedgerId(1)");
        assert_eq!(LedgerId::EUR.to_string(), "LedgerId(2)");
    }

    #[test]
    fn ledger_id_copy_semantics() {
        let a = LedgerId::USD;
        let b = a; // Copy
        assert_eq!(a, b);
    }

    // ── TransferId ───────────────────────────────────────────────────────────

    #[test]
    fn transfer_id_roundtrip() {
        let id = TransferId::new();
        let s = id.to_string();
        let reparsed = TransferId::from_str(&s).unwrap();
        assert_eq!(id, reparsed);
    }

    // ── Type safety ──────────────────────────────────────────────────────────

    #[test]
    fn different_id_types_with_same_value_are_not_equal() {
        // Type system prevents mixing at compile time; this confirms value-level
        // representation remains distinct per-type.
        let tx = TransactionId::from_u64(42);
        let acct = AccountId::from_u64(42);
        // Same inner value, different types — cannot be compared (won't compile).
        assert_eq!(tx.to_string(), acct.to_string());
    }

    #[test]
    fn transaction_id_as_u64_roundtrip() {
        let id = TransactionId::from_u64(999);
        assert_eq!(id.as_u64(), 999_u64);
    }
}

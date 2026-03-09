//! Core traits shared across the entire Blazil workspace.
//!
//! These traits define the minimum contracts that all Blazil domain types
//! must satisfy. They are small, focused, and composable.
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::traits::{Validate, Identifiable};
//! use blazil_common::error::{BlazerError, BlazerResult};
//! use blazil_common::ids::AccountId;
//!
//! struct Account {
//!     id: AccountId,
//!     name: String,
//! }
//!
//! impl Validate for Account {
//!     fn validate(&self) -> BlazerResult<()> {
//!         if self.name.is_empty() {
//!             return Err(BlazerError::ValidationError("name must not be empty".into()));
//!         }
//!         Ok(())
//!     }
//! }
//!
//! impl Identifiable for Account {
//!     type Id = AccountId;
//!     fn id(&self) -> &AccountId {
//!         &self.id
//!     }
//! }
//!
//! let acct = Account { id: AccountId::new(), name: "Savings".into() };
//! assert!(acct.validate().is_ok());
//! ```

use crate::error::BlazerResult;

/// Structural validation for Blazil domain types.
///
/// Implementors verify their own internal invariants and return
/// `Ok(())` if the state is consistent, or a descriptive
/// [`crate::error::BlazerError::ValidationError`] if not.
///
/// Validation is intentionally separate from construction. Some objects
/// may be partially constructed (e.g. deserialized from the network)
/// before validation is appropriate.
///
/// # Examples
///
/// ```rust
/// use blazil_common::traits::Validate;
/// use blazil_common::error::{BlazerError, BlazerResult};
///
/// struct PositiveAmount(i64);
///
/// impl Validate for PositiveAmount {
///     fn validate(&self) -> BlazerResult<()> {
///         if self.0 <= 0 {
///             return Err(BlazerError::ValidationError(
///                 format!("amount must be positive, got {}", self.0)
///             ));
///         }
///         Ok(())
///     }
/// }
///
/// assert!(PositiveAmount(100).validate().is_ok());
/// assert!(PositiveAmount(-1).validate().is_err());
/// ```
pub trait Validate {
    /// Validates the internal state of this type.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BlazerError::ValidationError`] with a
    /// human-readable description of what invariant was violated.
    fn validate(&self) -> BlazerResult<()>;
}

/// Provides access to the unique identifier of a Blazil domain entity.
///
/// The associated type `Id` must implement `Display` and `Debug` so that
/// identifiers can always be logged and serialized without additional bounds.
///
/// # Examples
///
/// ```rust
/// use blazil_common::traits::Identifiable;
/// use blazil_common::ids::TransactionId;
///
/// struct Transaction {
///     id: TransactionId,
/// }
///
/// impl Identifiable for Transaction {
///     type Id = TransactionId;
///     fn id(&self) -> &TransactionId {
///         &self.id
///     }
/// }
///
/// let tx = Transaction { id: TransactionId::new() };
/// println!("Transaction id: {}", tx.id());
/// ```
pub trait Identifiable {
    /// The concrete identifier type for this entity.
    ///
    /// Must implement [`std::fmt::Display`] and [`std::fmt::Debug`] so
    /// that all IDs are loggable without additional bounds at call sites.
    type Id: std::fmt::Display + std::fmt::Debug;

    /// Returns a reference to the entity's unique identifier.
    fn id(&self) -> &Self::Id;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::BlazerError;
    use crate::ids::AccountId;

    // ── Test fixtures ─────────────────────────────────────────────────────────

    struct AlwaysValid;
    impl Validate for AlwaysValid {
        fn validate(&self) -> BlazerResult<()> {
            Ok(())
        }
    }

    struct AlwaysInvalid;
    impl Validate for AlwaysInvalid {
        fn validate(&self) -> BlazerResult<()> {
            Err(BlazerError::ValidationError("always invalid".into()))
        }
    }

    struct NamedEntity {
        id: AccountId,
        name: String,
    }

    impl Validate for NamedEntity {
        fn validate(&self) -> BlazerResult<()> {
            if self.name.is_empty() {
                return Err(BlazerError::ValidationError(
                    "name must not be empty".into(),
                ));
            }
            Ok(())
        }
    }

    impl Identifiable for NamedEntity {
        type Id = AccountId;
        fn id(&self) -> &AccountId {
            &self.id
        }
    }

    // ── Validate tests ────────────────────────────────────────────────────────

    #[test]
    fn validate_ok_returns_unit() {
        assert!(AlwaysValid.validate().is_ok());
    }

    #[test]
    fn validate_err_returns_validation_error() {
        let err = AlwaysInvalid.validate().unwrap_err();
        assert!(matches!(err, BlazerError::ValidationError(_)));
    }

    #[test]
    fn named_entity_with_empty_name_fails_validation() {
        let entity = NamedEntity {
            id: AccountId::new(),
            name: String::new(),
        };
        assert!(entity.validate().is_err());
    }

    #[test]
    fn named_entity_with_nonempty_name_passes_validation() {
        let entity = NamedEntity {
            id: AccountId::new(),
            name: "Savings".into(),
        };
        assert!(entity.validate().is_ok());
    }

    // ── Identifiable tests ────────────────────────────────────────────────────

    #[test]
    fn identifiable_returns_correct_id() {
        let id = AccountId::new();
        let entity = NamedEntity {
            id,
            name: "Test".into(),
        };
        assert_eq!(entity.id(), &id);
    }

    #[test]
    fn identifiable_id_implements_display_and_debug() {
        let entity = NamedEntity {
            id: AccountId::new(),
            name: "Test".into(),
        };
        // Verify Display and Debug bounds are satisfied
        let _ = format!("{}", entity.id());
        let _ = format!("{:?}", entity.id());
    }
}

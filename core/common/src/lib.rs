//! # blazil-common
//!
//! Foundational type system for the Blazil workspace.
//!
//! Every other crate in the workspace depends on this crate. It provides:
//!
//! - **[`ids`]** — Opaque UUID-backed identity types for all domain entities.
//! - **[`amount`]** — Fixed-point monetary amounts with currency-aware arithmetic (no floats).
//! - **[`currency`]** — ISO 4217 currency code parsing and re-export.
//! - **[`timestamp`]** — Nanosecond-precision timestamps.
//! - **[`error`]** — [`BlazerError`] — the single unified error type for the workspace.
//! - **[`traits`]** — [`Validate`] and [`Identifiable`] — core domain contracts.
//!
//! # Design principle
//!
//! **Make illegal states unrepresentable.** If code compiles, it is
//! structurally correct — the type system enforces financial invariants
//! before a single byte hits the network or the ledger.

pub mod amount;
pub mod currency;
pub mod error;
pub mod ids;
pub mod timestamp;
pub mod traits;

// ── Convenience re-exports ──────────────────────────────────────────────────
//
// Consumers import from `blazil_common` directly without having to know
// which submodule a type lives in.

pub use amount::Amount;
pub use currency::{parse_currency, Currency};
pub use error::{BlazerError, BlazerResult};
pub use ids::{AccountId, LedgerId, TransactionId, TransferId};
pub use timestamp::Timestamp;
pub use traits::{Identifiable, Validate};

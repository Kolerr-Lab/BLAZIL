//! # Blazil Ledger
//!
//! TigerBeetle client abstraction layer for Blazil.
//!
//! Every debit and credit in the system flows through this crate.

pub mod account;
pub mod client;
pub mod convert;
pub mod double_entry;
pub mod mock;
pub mod transfer;

#[cfg(feature = "tigerbeetle-client")]
pub mod tigerbeetle;

pub use account::{Account, AccountFlags};
pub use client::LedgerClient;
pub use mock::InMemoryLedgerClient;
pub use transfer::{Transfer, TransferFlags};

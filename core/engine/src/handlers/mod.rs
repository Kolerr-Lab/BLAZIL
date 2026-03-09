//! Handler implementations for the Blazil engine pipeline.
//!
//! Each submodule contains one pipeline stage:
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`validation`] | Structural validation of transaction fields |
//! | [`risk`] | Limit and fraud signal checks |
//! | [`ledger`] | TigerBeetle commit via [`LedgerClient`][blazil_ledger::client::LedgerClient] |
//! | [`publish`] | Event stream publication (placeholder) |

pub mod ledger;
pub mod publish;
pub mod risk;
pub mod validation;

// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! blazil-screening — KYC/AML transaction screening infrastructure.
//!
//! Provides the compliance screening layer for the Blazil transaction engine,
//! designed to integrate at two points in the pipeline:
//!
//! 1. **Real-time** (`ScreeningMode::RealTime`): inline with a hard 50 ms
//!    deadline. On timeout, falls back to `Clear` and signals the caller to
//!    enqueue for batch re-screening.
//!
//! 2. **Batch** (`ScreeningMode::Batch`): asynchronous post-commit processing
//!    via an mpsc channel, consumed by a dedicated `BatchWorker` Tokio task.
//!
//! # Provider integration
//!
//! External providers (Sardine, Chainalysis, Elliptic) implement the
//! `TransactionScreener` trait. Wire-ready HTTP client skeletons live in
//! `providers/`; full implementation is gated on signed API contracts.
//!
//! # Hold / Release flow
//!
//! Transaction holds are persisted via the `HoldStore` trait. The
//! `InMemoryHoldStore` is provided for testing only — production requires a
//! TigerBeetle-backed implementation for durability and full audit trail.
//!
//! # SAR filing
//!
//! `SarReport::to_xml()` returns FinCEN SAR-compatible UTF-8 XML as
//! `Vec<u8>`. The caller owns all I/O (disk write, BSA E-Filing upload,
//! encryption).

pub mod batch;
pub mod error;
pub mod hold;
pub mod mock;
pub mod providers;
pub mod realtime;
pub mod sar;
pub mod screener;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export core types at crate root for ergonomic imports.
pub use error::ScreeningError;
pub use screener::TransactionScreener;
pub use types::{RiskLevel, ScreeningMode, ScreeningResult, TransactionEvent};

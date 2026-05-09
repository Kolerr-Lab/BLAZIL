// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! The `TransactionScreener` trait — the contract every provider must satisfy.

use async_trait::async_trait;

use crate::{ScreeningMode, ScreeningResult, TransactionEvent};

/// A compliance screening provider.
///
/// Implementations wrap external AML/KYC APIs (Sardine, Chainalysis, Elliptic)
/// or rule-based engines (see `MockScreener`). They are shared across async
/// tasks via `Arc<dyn TransactionScreener>` and must therefore be `Send + Sync`.
///
/// # Contract
///
/// - Implementations **must not panic**. Any infrastructure error should be
///   handled internally (log + fallback) rather than propagated as a panic,
///   since a panic in a screening call would abort a live transaction.
/// - In `RealTime` mode, the implementation should return within the 50 ms
///   deadline enforced by `RealTimeRouter`. Slow implementations will be
///   automatically timed out, but they should still minimise latency.
#[async_trait]
pub trait TransactionScreener: Send + Sync {
    /// Screens a transaction for AML/KYC compliance.
    ///
    /// `mode` indicates whether the call is on the critical real-time path
    /// or from the asynchronous batch worker. Providers may use this to
    /// choose between synchronous (low-latency) and enriched (higher-latency)
    /// screening strategies.
    async fn screen(&self, tx: &TransactionEvent, mode: ScreeningMode) -> ScreeningResult;

    /// Human-readable name of this provider, used in logs and SAR metadata.
    fn provider_name(&self) -> &'static str;
}

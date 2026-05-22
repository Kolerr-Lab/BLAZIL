// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Sardine AI — real-time fraud and AML screening provider.
//!
//! # Integration status: pending contract
//!
//! This module is wire-ready. Once the Sardine API contract is finalised,
//! implement:
//!
//! 1. `build_request(&TransactionEvent) -> SardineRequest` — serialise the
//!    event to Sardine's JSON payload format.
//!    Docs: https://docs.sardine.ai/docs/integrate-sardine/getting-started
//!
//! 2. `parse_response(body: Bytes) -> ScreeningResult` — deserialise Sardine's
//!    risk score and signals into a `ScreeningResult` variant.
//!
//! The `ProviderConfig` (`endpoint`, `api_key`, `timeout`, `max_retries`)
//! drives all HTTP behaviour. Auth is via the `Sardine-Client-Id` /
//! `Sardine-Secret-Key` header pair documented in the Sardine API reference.

use async_trait::async_trait;

use crate::{ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

/// Sardine AI screening provider.
pub struct SardineScreener {
    #[allow(dead_code)] // removed once HTTP client is wired up
    config: super::ProviderConfig,
}

impl SardineScreener {
    /// Creates a Sardine screener from the given provider config.
    pub fn new(config: super::ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl TransactionScreener for SardineScreener {
    async fn screen(&self, _tx: &TransactionEvent, _mode: ScreeningMode) -> ScreeningResult {
        // Integration status: pending API contract — see module-level docs.
        //
        // Implementation checklist (activate when contract is signed):
        //   [ ] Obtain API credentials from secrets manager.
        //   [ ] Implement `build_request` to map TransactionEvent → Sardine payload.
        //   [ ] POST to `self.config.endpoint` with auth headers and `self.config.timeout`.
        //   [ ] Retry up to `self.config.max_retries` on 5xx / timeout (exp backoff + jitter).
        //   [ ] Implement `parse_response` to map risk score → ScreeningResult.
        //   [ ] Add integration tests against Sardine sandbox.
        //
        // Safe-fail: hold the transaction pending manual compliance review.
        // Clearing an unscreened transaction would violate AML obligations.
        ScreeningResult::Hold {
            reason: "Sardine AI screening provider not yet configured \
                     (pending API contract). \
                     Transaction held for manual compliance review."
                .to_owned(),
            review_required: true,
        }
    }

    fn provider_name(&self) -> &'static str {
        "sardine"
    }
}

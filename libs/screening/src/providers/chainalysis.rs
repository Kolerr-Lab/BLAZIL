// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Chainalysis — blockchain analytics and crypto AML screening provider.
//!
//! # Integration status: pending contract
//!
//! This module is wire-ready. Once the Chainalysis API contract is finalised,
//! implement:
//!
//! 1. `build_request(&TransactionEvent) -> ChainalysisRequest` — map the event
//!    to Chainalysis's KYT (Know Your Transaction) API payload.
//!    Docs: https://docs.chainalysis.com/api/kyt/
//!
//! 2. `parse_response(body: Bytes) -> ScreeningResult` — map the KYT alert
//!    level (`SEVERE`, `HIGH`, `MEDIUM`, `LOW`) to `ScreeningResult` variants.
//!
//! Auth is via the `Token` header: `Token {api_key}`.

use async_trait::async_trait;

use crate::{ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

/// Chainalysis KYT screening provider.
pub struct ChainalysisScreener {
    #[allow(dead_code)] // removed once HTTP client is wired up
    config: super::ProviderConfig,
}

impl ChainalysisScreener {
    /// Creates a Chainalysis screener from the given provider config.
    pub fn new(config: super::ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl TransactionScreener for ChainalysisScreener {
    async fn screen(&self, _tx: &TransactionEvent, _mode: ScreeningMode) -> ScreeningResult {
        // TODO(chainalysis): implement once API contract is signed.
        //
        // Checklist:
        //   [ ] Obtain API credentials from secrets manager.
        //   [ ] Implement `build_request` to map TransactionEvent → KYT payload.
        //   [ ] POST to `self.config.endpoint` with `Token {api_key}` header.
        //   [ ] Map KYT alert levels: SEVERE → Reject, HIGH → Hold, MEDIUM → Flag.
        //   [ ] Add integration tests against Chainalysis sandbox.
        todo!("Chainalysis provider: pending API contract — see module docs")
    }

    fn provider_name(&self) -> &'static str {
        "chainalysis"
    }
}

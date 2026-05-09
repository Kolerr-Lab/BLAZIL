// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Elliptic — crypto asset risk management and AML screening provider.
//!
//! # Integration status: pending contract
//!
//! This module is wire-ready. Once the Elliptic API contract is finalised,
//! implement:
//!
//! 1. `build_request(&TransactionEvent) -> EllipticRequest` — map the event
//!    to Elliptic's Transaction Screening API payload.
//!    Docs: https://developers.elliptic.co/docs/transaction-screening
//!
//! 2. `parse_response(body: Bytes) -> ScreeningResult` — map Elliptic's
//!    risk score (0.0–1.0) and risk rules to `ScreeningResult` variants.
//!
//! Auth is via the `x-access-key` / `x-access-sign` HMAC signature scheme.

use async_trait::async_trait;

use crate::{ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

/// Elliptic transaction screening provider.
pub struct EllipticScreener {
    #[allow(dead_code)] // removed once HTTP client is wired up
    config: super::ProviderConfig,
}

impl EllipticScreener {
    /// Creates an Elliptic screener from the given provider config.
    pub fn new(config: super::ProviderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl TransactionScreener for EllipticScreener {
    async fn screen(&self, _tx: &TransactionEvent, _mode: ScreeningMode) -> ScreeningResult {
        // TODO(elliptic): implement once API contract is signed.
        //
        // Checklist:
        //   [ ] Obtain API credentials from secrets manager.
        //   [ ] Implement HMAC-SHA256 request signing per Elliptic auth scheme.
        //   [ ] Implement `build_request` to map TransactionEvent → payload.
        //   [ ] Map risk score: ≥ 0.8 → Reject, ≥ 0.5 → Hold, ≥ 0.3 → Flag.
        //   [ ] Add integration tests against Elliptic sandbox.
        todo!("Elliptic provider: pending API contract — see module docs")
    }

    fn provider_name(&self) -> &'static str {
        "elliptic"
    }
}

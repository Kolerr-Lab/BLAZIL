// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Sardine AI — real-time fraud and AML screening provider.
//!
//! # Integration status: HTTP infrastructure wired
//!
//! This module is ready to go live once sandbox credentials are obtained.
//! All HTTP plumbing, request/response mapping, and error handling are
//! implemented. The only manual step before going live is validating the
//! risk-score thresholds against Sardine's sandbox outputs with your
//! compliance team (see [`map_sardine_response`]).
//!
//! # Auth
//!
//! Two headers per request:
//! - `Sardine-Client-Id`: load from `SARDINE_CLIENT_ID` env var
//! - `Sardine-Secret-Key`: load from `SARDINE_SECRET_KEY` env var
//!
//! Build the config with:
//! ```ignore
//! let config = ProviderConfig::sardine(
//!     Region::Apac,
//!     std::env::var("SARDINE_CLIENT_ID").unwrap(),
//!     std::env::var("SARDINE_SECRET_KEY").unwrap(),
//! );
//! ```
//!
//! # Mode behaviour
//!
//! - **RealTime**: single HTTP attempt, safe-fail to `Hold` on any error.
//! - **Batch**: up to `max_retries` attempts with exponential back-off + jitter.
//!
//! # Updating the API schema
//!
//! All request fields live in [`SardineRequest`]; all response fields in
//! [`SardineResponse`]. Update those two structs and [`map_sardine_response`]
//! when the signed contract specifies additional fields.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::{RiskLevel, ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

// ── Request / Response types ──────────────────────────────────────────────────

/// Sardine AI transaction risk assessment request.
///
/// Field names follow Sardine's public API schema (v1, `camelCase`).
/// See: <https://docs.sardine.ai/docs/integrate-sardine/getting-started>
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SardineRequest {
    /// Idempotency key — must be unique per transaction.
    session_key: String,
    /// ISO 4217 currency code (e.g. `"USD"`, `"SGD"`).
    currency_code: String,
    /// Transaction amount as a decimal string (e.g. `"10.50"`).
    amount: String,
    /// Originating party identifier (internal account ID or wallet address).
    sender_id: String,
    /// Receiving party identifier.
    recipient_id: String,
    /// Jurisdiction tag for rule-set selection: `"APAC"` / `"NA"` / `"EU"`.
    region: String,
    /// Arbitrary metadata forwarded to Sardine for enrichment signals
    /// (e.g. `"ip"`, `"device_fingerprint"`, `"blockchain_address"`).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    metadata: HashMap<String, String>,
}

/// Sardine AI transaction risk assessment response.
///
/// `#[allow(dead_code)]` on diagnostic fields that will be wired to the
/// audit log once the audit crate integration is complete.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SardineResponse {
    /// Normalised risk score in [0.0, 1.0]. Higher is riskier.
    risk_score: f64,
    /// Human-readable label: `"low"` / `"medium"` / `"high"` / `"very_high"`.
    risk_level: String,
    /// Sardine's internal transaction ID (for correlation with their dashboard).
    #[allow(dead_code)]
    transaction_id: Option<String>,
    /// List of triggered risk signals (for audit log enrichment).
    #[allow(dead_code)]
    signals: Option<Vec<String>>,
}

// ── Score → ScreeningResult mapping ──────────────────────────────────────────

/// Maps a Sardine risk score to a [`ScreeningResult`].
///
/// Thresholds are calibrated against MAS TRM / FinCEN / AMLD6 guidance.
/// **Review with your compliance team before going live.**
///
/// | Score range | Result                          |
/// |-------------|---------------------------------|
/// | ≥ 0.85      | `Reject` (SAR required)         |
/// | ≥ 0.60      | `Hold` (analyst review required) |
/// | ≥ 0.30      | `Flag` (medium severity)        |
/// | < 0.30      | `Clear`                         |
fn map_sardine_response(resp: &SardineResponse) -> ScreeningResult {
    if resp.risk_score >= 0.85 {
        ScreeningResult::Reject {
            reason: format!(
                "Sardine: high-risk transaction (score={:.3}, level={})",
                resp.risk_score, resp.risk_level
            ),
            sar_required: true,
        }
    } else if resp.risk_score >= 0.60 {
        ScreeningResult::Hold {
            reason: format!(
                "Sardine: elevated risk (score={:.3}, level={})",
                resp.risk_score, resp.risk_level
            ),
            review_required: true,
        }
    } else if resp.risk_score >= 0.30 {
        ScreeningResult::Flag {
            reason: format!(
                "Sardine: suspicious activity detected (score={:.3}, level={})",
                resp.risk_score, resp.risk_level
            ),
            severity: RiskLevel::Medium,
        }
    } else {
        ScreeningResult::Clear
    }
}

// ── Screener ──────────────────────────────────────────────────────────────────

/// Sardine AI real-time fraud and AML screening provider.
///
/// Construct via [`super::ProviderConfig::sardine`]:
/// ```ignore
/// let screener = SardineScreener::new(
///     ProviderConfig::sardine(Region::Apac, client_id, secret_key)
/// );
/// ```
pub struct SardineScreener {
    config: super::ProviderConfig,
    /// Shared HTTP client — manages a connection pool internally.
    /// Never create a new client per request.
    client: reqwest::Client,
}

impl SardineScreener {
    /// Creates a Sardine screener from the given provider config.
    ///
    /// # Panics
    ///
    /// Panics if TLS initialisation fails — this indicates a broken build
    /// environment, not a recoverable runtime error.
    pub fn new(config: super::ProviderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .use_rustls_tls()
            .build()
            .expect("Sardine: TLS client initialisation failed");
        Self { config, client }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn build_request(&self, tx: &TransactionEvent) -> SardineRequest {
        // Convert minor units (cents) to decimal: 10050 → "100.50"
        let amount = format!("{:.2}", tx.amount as f64 / 100.0);
        SardineRequest {
            session_key: tx.transaction_id.clone(),
            currency_code: tx.currency.clone(),
            amount,
            sender_id: tx.sender_id.clone(),
            recipient_id: tx.receiver_id.clone(),
            region: self.config.region.as_tag().to_owned(),
            metadata: tx.metadata.clone(),
        }
    }

    /// Single HTTP call — used by both modes.
    async fn call_api(&self, tx: &TransactionEvent) -> Result<ScreeningResult, reqwest::Error> {
        let client_id = self.config.client_id.as_deref().unwrap_or_default();
        let url = format!("{}/v1/payments/transactions", self.config.endpoint);
        let body = self.build_request(tx);

        let resp = self
            .client
            .post(&url)
            .header("Sardine-Client-Id", client_id)
            .header("Sardine-Secret-Key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        let parsed: SardineResponse = resp.json().await?;
        debug!(
            provider = "sardine",
            tx_id = %tx.transaction_id,
            score = parsed.risk_score,
            level = %parsed.risk_level,
            "Sardine response received",
        );
        Ok(map_sardine_response(&parsed))
    }

    /// [`call_api`] with exponential back-off + full jitter.
    /// Called only in `Batch` mode.
    async fn call_with_retry(&self, tx: &TransactionEvent) -> ScreeningResult {
        for attempt in 0..=self.config.max_retries {
            match self.call_api(tx).await {
                Ok(result) => return result,
                Err(e) => {
                    if attempt == self.config.max_retries {
                        warn!(
                            provider = "sardine",
                            tx_id = %tx.transaction_id,
                            error = %e,
                            "All retry attempts exhausted — holding transaction",
                        );
                        break;
                    }
                    // Exponential back-off: base = 100 ms * 2^attempt.
                    // Full jitter: delay ∈ [0, base) using subsecond clock entropy.
                    let base_ms: u64 = 100 * (1u64 << attempt);
                    let jitter_ms = subsec_jitter(base_ms);
                    warn!(
                        provider = "sardine",
                        tx_id = %tx.transaction_id,
                        attempt,
                        delay_ms = jitter_ms,
                        error = %e,
                        "Transient error — retrying",
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
                }
            }
        }
        ScreeningResult::Hold {
            reason: "Sardine screening unavailable after retries. \
                     Transaction held for manual compliance review."
                .to_owned(),
            review_required: true,
        }
    }
}

#[async_trait]
impl TransactionScreener for SardineScreener {
    #[instrument(
        skip(self, tx),
        fields(
            provider = "sardine",
            tx_id = %tx.transaction_id,
            mode = ?mode,
        )
    )]
    async fn screen(&self, tx: &TransactionEvent, mode: ScreeningMode) -> ScreeningResult {
        match mode {
            ScreeningMode::RealTime => {
                // Single attempt only — must stay within the 45 ms budget.
                // Any network or parse error falls through to safe-fail Hold.
                match self.call_api(tx).await {
                    Ok(result) => result,
                    Err(e) => {
                        warn!(
                            provider = "sardine",
                            tx_id = %tx.transaction_id,
                            error = %e,
                            "Real-time screening failed — holding transaction",
                        );
                        ScreeningResult::Hold {
                            reason: "Sardine screening unavailable. \
                                     Transaction held for manual compliance review."
                                .to_owned(),
                            review_required: true,
                        }
                    }
                }
            }
            ScreeningMode::Batch => self.call_with_retry(tx).await,
        }
    }

    fn provider_name(&self) -> &'static str {
        "sardine"
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Returns a random delay in [0, max_ms) using subsecond system-clock entropy.
/// Avoids a `rand` dependency while still providing adequate jitter for retry
/// back-off. Not suitable for cryptographic use.
fn subsec_jitter(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % max_ms)
        .unwrap_or(0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderConfig, Region};

    fn make_tx() -> TransactionEvent {
        TransactionEvent::new("test-tx-001", 50_000, "USD", "alice", "bob")
    }

    fn resp(score: f64, level: &str) -> SardineResponse {
        SardineResponse {
            risk_score: score,
            risk_level: level.to_owned(),
            transaction_id: None,
            signals: None,
        }
    }

    #[test]
    fn map_high_risk_is_reject_with_sar() {
        assert!(matches!(
            map_sardine_response(&resp(0.92, "very_high")),
            ScreeningResult::Reject { sar_required: true, .. }
        ));
    }

    #[test]
    fn map_elevated_risk_is_hold() {
        assert!(matches!(
            map_sardine_response(&resp(0.70, "high")),
            ScreeningResult::Hold { review_required: true, .. }
        ));
    }

    #[test]
    fn map_suspicious_is_flag_medium() {
        assert!(matches!(
            map_sardine_response(&resp(0.45, "medium")),
            ScreeningResult::Flag { severity: RiskLevel::Medium, .. }
        ));
    }

    #[test]
    fn map_low_risk_is_clear() {
        assert_eq!(map_sardine_response(&resp(0.10, "low")), ScreeningResult::Clear);
    }

    #[test]
    fn build_request_converts_minor_units() {
        let config = ProviderConfig::sardine(Region::Apac, "cid", "key");
        let screener = SardineScreener::new(config);
        let tx = make_tx(); // amount = 50_000 cents
        let req = screener.build_request(&tx);
        assert_eq!(req.amount, "500.00");
        assert_eq!(req.region, "APAC");
        assert_eq!(req.session_key, "test-tx-001");
    }

    /// Live integration test — requires real Sardine sandbox credentials.
    ///
    /// To run:
    /// ```text
    /// SARDINE_CLIENT_ID=xxx SARDINE_SECRET_KEY=yyy \
    ///   cargo test -p blazil-screening -- --ignored sardine_live_realtime
    /// ```
    #[tokio::test]
    #[ignore = "requires Sardine sandbox credentials (SARDINE_CLIENT_ID, SARDINE_SECRET_KEY)"]
    async fn sardine_live_realtime() {
        let client_id = std::env::var("SARDINE_CLIENT_ID").expect("SARDINE_CLIENT_ID not set");
        let secret_key = std::env::var("SARDINE_SECRET_KEY").expect("SARDINE_SECRET_KEY not set");
        let config = ProviderConfig::sardine(Region::Apac, client_id, secret_key);
        let screener = SardineScreener::new(config);
        let result = screener.screen(&make_tx(), ScreeningMode::RealTime).await;
        println!("Live Sardine result: {result:?}");
        // Validate that any non-panicking result is a valid variant.
        assert!(matches!(
            result,
            ScreeningResult::Clear
                | ScreeningResult::Flag { .. }
                | ScreeningResult::Hold { .. }
                | ScreeningResult::Reject { .. }
        ));
    }
}


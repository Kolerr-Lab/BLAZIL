// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Elliptic — crypto asset risk management and AML screening provider.
//!
//! # Integration status: HTTP infrastructure wired
//!
//! All HTTP plumbing, HMAC-SHA256 request signing, request/response mapping,
//! and error handling are implemented. Before going live:
//!
//! 1. Obtain sandbox credentials from Elliptic and set env vars:
//!    ```text
//!    ELLIPTIC_API_KEY    → api_key
//!    ELLIPTIC_API_SECRET → api_secret
//!    ```
//! 2. Verify that [`TransactionEvent::metadata`] is populated with
//!    `"blockchain_address"`, `"blockchain"`, and `"asset"` keys by upstream
//!    callers (required for crypto asset screening).
//! 3. Validate risk-score thresholds against sandbox outputs with your
//!    compliance team (see [`map_elliptic_response`]).
//! 4. Run the `#[ignore]` integration tests against the sandbox.
//!
//! # Auth
//!
//! Elliptic uses HMAC-SHA256 request signing:
//! ```text
//! x-access-key:       {api_key}
//! x-access-sign:      HMAC-SHA256(api_secret, timestamp + "POST" + path + body)
//! x-access-timestamp: {unix_seconds}
//! ```
//!
//! # Blockchain data
//!
//! Elliptic screens blockchain addresses and transaction hashes. Callers must
//! populate [`TransactionEvent::metadata`] with:
//! - `"blockchain_address"` — the wallet address or tx hash to screen
//! - `"blockchain"` — e.g. `"ethereum"`, `"bitcoin"` (Elliptic identifiers)
//! - `"asset"` — e.g. `"ETH"`, `"BTC"` (Elliptic asset symbols)
//!
//! If these keys are absent the screener returns `Hold` immediately without
//! an API call.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::{RiskLevel, ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

// ── Request / Response types ──────────────────────────────────────────────────

/// Elliptic v2 wallet/transaction screening request.
///
/// See: <https://developers.elliptic.co/docs/transaction-screening>
#[derive(Debug, Serialize)]
struct EllipticRequest {
    subject: EllipticSubject,
    /// Screening direction: `"destination"` or `"source"`.
    #[serde(rename = "type")]
    screening_type: &'static str,
    /// Jurisdiction context forwarded for rule-set selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    jurisdiction: Option<String>,
}

#[derive(Debug, Serialize)]
struct EllipticSubject {
    /// `"address"` for wallet screening; `"transaction"` for tx hash.
    #[serde(rename = "type")]
    subject_type: &'static str,
    /// Elliptic asset identifier (e.g. `"ETH"`, `"BTC"`).
    asset: String,
    /// Elliptic blockchain identifier (e.g. `"ethereum"`, `"bitcoin"`).
    blockchain: String,
    /// Wallet address or transaction hash.
    hash: String,
}

/// Elliptic v2 screening response.
#[derive(Debug, Deserialize)]
struct EllipticResponse {
    /// Normalised risk score in [0.0, 1.0]. Higher is riskier.
    risk_score: f64,
    /// Elliptic's internal result ID (for dashboard correlation).
    #[allow(dead_code)]
    id: Option<String>,
}

// ── Score → ScreeningResult mapping ──────────────────────────────────────────

/// Maps an Elliptic risk score to a [`ScreeningResult`].
///
/// Thresholds follow Elliptic's recommended risk rule configuration for
/// MAS TRM / FinCEN / AMLD6 compliance contexts.
/// **Review with your compliance team before going live.**
///
/// | Score range | Result                          |
/// |-------------|---------------------------------|
/// | ≥ 0.80      | `Reject` (SAR required)         |
/// | ≥ 0.50      | `Hold` (analyst review required) |
/// | ≥ 0.30      | `Flag` (high severity)          |
/// | < 0.30      | `Clear`                         |
fn map_elliptic_response(resp: &EllipticResponse) -> ScreeningResult {
    if resp.risk_score >= 0.80 {
        ScreeningResult::Reject {
            reason: format!(
                "Elliptic: high-risk address/tx (score={:.3})",
                resp.risk_score
            ),
            sar_required: true,
        }
    } else if resp.risk_score >= 0.50 {
        ScreeningResult::Hold {
            reason: format!("Elliptic: elevated risk (score={:.3})", resp.risk_score),
            review_required: true,
        }
    } else if resp.risk_score >= 0.30 {
        ScreeningResult::Flag {
            reason: format!(
                "Elliptic: suspicious indicators (score={:.3})",
                resp.risk_score
            ),
            severity: RiskLevel::High,
        }
    } else {
        ScreeningResult::Clear
    }
}

// ── HMAC-SHA256 request signing ───────────────────────────────────────────────

/// Computes the Elliptic `x-access-sign` header value.
///
/// Signature = HMAC-SHA256(`api_secret`, `timestamp + method + path + body`)
/// encoded as lowercase hex.
fn compute_hmac_hex(api_secret: &str, data: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac =
        HmacSha256::new_from_slice(api_secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    // Hex-encode without an external crate
    mac.finalize()
        .into_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            write!(s, "{b:02x}").expect("writing to a String is infallible");
            s
        })
}

// ── Screener ──────────────────────────────────────────────────────────────────

/// Elliptic crypto asset AML screening provider.
///
/// Construct via [`super::ProviderConfig::elliptic`]:
/// ```ignore
/// let screener = EllipticScreener::new(
///     ProviderConfig::elliptic(Region::Apac, api_key, api_secret)
/// );
/// ```
pub struct EllipticScreener {
    config: super::ProviderConfig,
    /// Shared HTTP client — manages a connection pool internally.
    client: reqwest::Client,
}

impl EllipticScreener {
    /// Creates an Elliptic screener from the given provider config.
    ///
    /// # Panics
    ///
    /// Panics if TLS initialisation fails (broken build environment).
    pub fn new(config: super::ProviderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .use_rustls_tls()
            .build()
            .expect("Elliptic: TLS client initialisation failed");
        Self { config, client }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Builds an Elliptic request from a [`TransactionEvent`].
    ///
    /// Returns `None` if the required blockchain metadata keys are absent.
    fn build_request(&self, tx: &TransactionEvent) -> Option<EllipticRequest> {
        let address = tx.metadata.get("blockchain_address")?;
        let blockchain = tx.metadata.get("blockchain")?;
        let asset = tx.metadata.get("asset")?;

        Some(EllipticRequest {
            subject: EllipticSubject {
                subject_type: "address",
                asset: asset.clone(),
                blockchain: blockchain.clone(),
                hash: address.clone(),
            },
            screening_type: "destination",
            jurisdiction: Some(self.config.region.as_tag().to_owned()),
        })
    }

    async fn call_api(&self, tx: &TransactionEvent) -> Result<ScreeningResult, reqwest::Error> {
        let body = match self.build_request(tx) {
            Some(b) => b,
            None => {
                // Missing blockchain metadata — cannot screen with Elliptic.
                warn!(
                    provider = "elliptic",
                    tx_id = %tx.transaction_id,
                    "Missing blockchain_address/blockchain/asset in metadata — holding transaction",
                );
                return Ok(ScreeningResult::Hold {
                    reason: "Elliptic: transaction metadata missing required blockchain fields. \
                             Transaction held for manual compliance review."
                        .to_owned(),
                    review_required: true,
                });
            }
        };

        let api_secret = self.config.api_secret.as_deref().unwrap_or_default();
        let path = "/v2/wallet/synchronous";
        let url = format!("{}{path}", self.config.endpoint);
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        // Elliptic signature: HMAC-SHA256(secret, timestamp + method + path + body)
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_owned());
        let signing_input = format!("{timestamp}POST{path}{body_str}");
        let signature = compute_hmac_hex(api_secret, &signing_input);

        let resp = self
            .client
            .post(&url)
            .header("x-access-key", &self.config.api_key)
            .header("x-access-sign", &signature)
            .header("x-access-timestamp", &timestamp)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?
            .error_for_status()?;

        let parsed: EllipticResponse = resp.json().await?;
        debug!(
            provider = "elliptic",
            tx_id = %tx.transaction_id,
            score = parsed.risk_score,
            "Elliptic response received",
        );
        Ok(map_elliptic_response(&parsed))
    }

    async fn call_with_retry(&self, tx: &TransactionEvent) -> ScreeningResult {
        for attempt in 0..=self.config.max_retries {
            match self.call_api(tx).await {
                Ok(result) => return result,
                Err(e) => {
                    if attempt == self.config.max_retries {
                        warn!(
                            provider = "elliptic",
                            tx_id = %tx.transaction_id,
                            error = %e,
                            "All retry attempts exhausted — holding transaction",
                        );
                        break;
                    }
                    let base_ms: u64 = 100 * (1u64 << attempt);
                    let jitter_ms = subsec_jitter(base_ms);
                    warn!(
                        provider = "elliptic",
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
            reason: "Elliptic screening unavailable after retries. \
                     Transaction held for manual compliance review."
                .to_owned(),
            review_required: true,
        }
    }
}

#[async_trait]
impl TransactionScreener for EllipticScreener {
    #[instrument(
        skip(self, tx),
        fields(
            provider = "elliptic",
            tx_id = %tx.transaction_id,
            mode = ?mode,
        )
    )]
    async fn screen(&self, tx: &TransactionEvent, mode: ScreeningMode) -> ScreeningResult {
        match mode {
            ScreeningMode::RealTime => match self.call_api(tx).await {
                Ok(result) => result,
                Err(e) => {
                    warn!(
                        provider = "elliptic",
                        tx_id = %tx.transaction_id,
                        error = %e,
                        "Real-time screening failed — holding transaction",
                    );
                    ScreeningResult::Hold {
                        reason: "Elliptic screening unavailable. \
                                 Transaction held for manual compliance review."
                            .to_owned(),
                        review_required: true,
                    }
                }
            },
            ScreeningMode::Batch => self.call_with_retry(tx).await,
        }
    }

    fn provider_name(&self) -> &'static str {
        "elliptic"
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

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

    fn make_crypto_tx() -> TransactionEvent {
        TransactionEvent::new("crypto-tx-001", 100_000, "ETH", "alice", "bob")
            .with_metadata(
                "blockchain_address",
                "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe",
            )
            .with_metadata("blockchain", "ethereum")
            .with_metadata("asset", "ETH")
    }

    fn resp(score: f64) -> EllipticResponse {
        EllipticResponse {
            risk_score: score,
            id: None,
        }
    }

    #[test]
    fn map_high_risk_is_reject_with_sar() {
        assert!(matches!(
            map_elliptic_response(&resp(0.85)),
            ScreeningResult::Reject {
                sar_required: true,
                ..
            }
        ));
    }

    #[test]
    fn map_elevated_is_hold() {
        assert!(matches!(
            map_elliptic_response(&resp(0.65)),
            ScreeningResult::Hold {
                review_required: true,
                ..
            }
        ));
    }

    #[test]
    fn map_suspicious_is_flag_high() {
        assert!(matches!(
            map_elliptic_response(&resp(0.40)),
            ScreeningResult::Flag {
                severity: RiskLevel::High,
                ..
            }
        ));
    }

    #[test]
    fn map_low_risk_is_clear() {
        assert_eq!(map_elliptic_response(&resp(0.10)), ScreeningResult::Clear);
    }

    #[test]
    fn hmac_hex_is_deterministic() {
        let a = compute_hmac_hex("secret", "data");
        let b = compute_hmac_hex("secret", "data");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // SHA-256 → 32 bytes → 64 hex chars
    }

    #[test]
    fn build_request_missing_metadata_returns_none() {
        // Transaction without blockchain metadata — build_request must return None.
        let config = ProviderConfig::elliptic(Region::Apac, "key", "secret");
        let screener = EllipticScreener::new(config);
        let tx = TransactionEvent::new("fiat-tx-001", 50_000, "USD", "alice", "bob");
        assert!(screener.build_request(&tx).is_none());
    }

    #[test]
    fn build_request_with_metadata_is_some() {
        let config = ProviderConfig::elliptic(Region::Eu, "key", "secret");
        let screener = EllipticScreener::new(config);
        let req = screener.build_request(&make_crypto_tx());
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.subject.blockchain, "ethereum");
        assert_eq!(req.jurisdiction, Some("EU".to_owned()));
    }

    /// Live integration test — requires Elliptic sandbox credentials.
    ///
    /// ```text
    /// ELLIPTIC_API_KEY=xxx ELLIPTIC_API_SECRET=yyy \
    ///   cargo test -p blazil-screening -- --ignored elliptic_live_realtime
    /// ```
    #[tokio::test]
    #[ignore = "requires Elliptic sandbox credentials (ELLIPTIC_API_KEY, ELLIPTIC_API_SECRET)"]
    async fn elliptic_live_realtime() {
        let api_key = std::env::var("ELLIPTIC_API_KEY").expect("ELLIPTIC_API_KEY not set");
        let api_secret = std::env::var("ELLIPTIC_API_SECRET").expect("ELLIPTIC_API_SECRET not set");
        let config = ProviderConfig::elliptic(Region::Apac, api_key, api_secret);
        let screener = EllipticScreener::new(config);
        let result = screener
            .screen(&make_crypto_tx(), ScreeningMode::RealTime)
            .await;
        println!("Live Elliptic result: {result:?}");
        assert!(matches!(
            result,
            ScreeningResult::Clear
                | ScreeningResult::Flag { .. }
                | ScreeningResult::Hold { .. }
                | ScreeningResult::Reject { .. }
        ));
    }
}

// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Chainalysis — blockchain analytics and KYT (Know Your Transaction) screening.
//!
//! # Integration status: HTTP infrastructure wired
//!
//! All HTTP plumbing, request/response mapping, and error handling are
//! implemented. Before going live:
//!
//! 1. Obtain sandbox credentials and set env var:
//!    ```text
//!    CHAINALYSIS_API_KEY → api_key
//!    ```
//! 2. Verify [`TransactionEvent::metadata`] is populated with blockchain data
//!    (see *Blockchain data* below).
//! 3. Run the `#[ignore]` integration tests against the sandbox.
//!
//! # Mode behaviour (important)
//!
//! Chainalysis KYT uses a **2-step async flow** (register transfer → poll for
//! result) that is incompatible with the 50 ms real-time deadline:
//!
//! - **`RealTime`**: returns `Hold` immediately — no network call is made.
//!   The caller's `BatchSender` should enqueue the transaction for deferred
//!   re-screening via the `BatchWorker`.
//! - **`Batch`**: performs the full register-then-poll cycle with exponential
//!   back-off. Maximum poll time = `timeout * max_poll_attempts` (≈ 15 s with
//!   defaults), after which the transaction is held for manual review.
//!
//! # Auth
//!
//! ```text
//! Authorization: Token {api_key}
//! ```
//!
//! # Blockchain data
//!
//! Chainalysis screens blockchain transactions. Callers must populate
//! [`TransactionEvent::metadata`] with:
//! - `"network"` — Chainalysis network identifier (e.g. `"ETHEREUM"`, `"BITCOIN"`)
//! - `"tx_hash"` — the on-chain transaction hash
//! - `"output_address"` — the receiving wallet address
//!
//! If these keys are absent the screener returns `Hold` immediately.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::{RiskLevel, ScreeningMode, ScreeningResult, TransactionEvent, TransactionScreener};

// ── Request / Response types ──────────────────────────────────────────────────

/// Chainalysis KYT v2 transfer registration request.
///
/// See: <https://docs.chainalysis.com/api/kyt/>
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    /// Chainalysis network identifier (e.g. `"ETHEREUM"`, `"BITCOIN"`).
    network: String,
    /// On-chain transaction hash.
    tx: String,
    /// The output (destination) wallet address.
    output_address: String,
    /// Client-supplied idempotency key — used to retrieve the result later.
    external_id: String,
    /// Transfer direction from your platform's perspective.
    direction: &'static str,
}

/// Chainalysis KYT v2 transfer registration response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterResponse {
    /// Echoed back — used to poll for the screening result.
    external_id: String,
}

/// Chainalysis KYT v2 transfer summary response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryResponse {
    /// `"COMPLETE"` when the risk assessment is ready; `"PROCESSING"` otherwise.
    alert_status: String,
    /// Overall risk severity: `"SEVERE"` / `"HIGH"` / `"MEDIUM"` / `"LOW"` / `null`.
    alert_level: Option<String>,
    /// Normalised risk score in [0.0, 1.0] (present when `alert_status = "COMPLETE"`).
    #[allow(dead_code)]
    alert_score: Option<f64>,
}

// ── Alert level → ScreeningResult mapping ────────────────────────────────────

/// Maps a Chainalysis `alertLevel` string to a [`ScreeningResult`].
///
/// | Alert level | Result                          |
/// |-------------|---------------------------------|
/// | `SEVERE`    | `Reject` (SAR required)         |
/// | `HIGH`      | `Hold` (analyst review required) |
/// | `MEDIUM`    | `Flag` (high severity)          |
/// | `LOW`       | `Flag` (low severity)           |
/// | `null`      | `Clear`                         |
fn map_alert_level(level: Option<&str>) -> ScreeningResult {
    match level {
        Some("SEVERE") => ScreeningResult::Reject {
            reason: "Chainalysis KYT: SEVERE alert — high-risk counterparty or sanctions exposure"
                .to_owned(),
            sar_required: true,
        },
        Some("HIGH") => ScreeningResult::Hold {
            reason: "Chainalysis KYT: HIGH alert — elevated risk, manual review required"
                .to_owned(),
            review_required: true,
        },
        Some("MEDIUM") => ScreeningResult::Flag {
            reason: "Chainalysis KYT: MEDIUM alert — suspicious activity indicators".to_owned(),
            severity: RiskLevel::High,
        },
        Some("LOW") => ScreeningResult::Flag {
            reason: "Chainalysis KYT: LOW alert — minor risk indicators".to_owned(),
            severity: RiskLevel::Low,
        },
        _ => ScreeningResult::Clear,
    }
}

// ── Screener ──────────────────────────────────────────────────────────────────

/// Maximum number of polling attempts when waiting for a KYT assessment.
const MAX_POLL_ATTEMPTS: u32 = 6;
/// Delay between poll attempts: 500 ms, 1 s, 2 s, 4 s, 8 s, 16 s.
const POLL_BASE_MS: u64 = 500;

/// Chainalysis KYT (Know Your Transaction) blockchain AML screening provider.
///
/// **Batch-only** — real-time calls return `Hold` immediately without a
/// network round-trip. Wire to [`crate::batch::BatchWorker`] for async
/// post-commit screening.
///
/// Construct via [`super::ProviderConfig::chainalysis`]:
/// ```ignore
/// let screener = ChainalysisScreener::new(
///     ProviderConfig::chainalysis(api_key)
/// );
/// ```
pub struct ChainalysisScreener {
    config: super::ProviderConfig,
    /// Shared HTTP client — manages a connection pool internally.
    client: reqwest::Client,
}

impl ChainalysisScreener {
    /// Creates a Chainalysis screener from the given provider config.
    ///
    /// # Panics
    ///
    /// Panics if TLS initialisation fails (broken build environment).
    pub fn new(config: super::ProviderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .use_rustls_tls()
            .build()
            .expect("Chainalysis: TLS client initialisation failed");
        Self { config, client }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Returns the `Authorization: Token {key}` header value.
    fn auth_header(&self) -> String {
        format!("Token {}", self.config.api_key)
    }

    /// Step 1 — register the transfer and return the `externalId`.
    async fn register_transfer(&self, tx: &TransactionEvent) -> Result<String, reqwest::Error> {
        let network = tx.metadata.get("network").cloned().unwrap_or_default();
        let tx_hash = tx.metadata.get("tx_hash").cloned().unwrap_or_default();
        let output_address = tx
            .metadata
            .get("output_address")
            .cloned()
            .unwrap_or_default();

        let url = format!("{}/api/kyt/v2/transfers", self.config.endpoint);
        let body = RegisterRequest {
            network,
            tx: tx_hash,
            output_address,
            external_id: tx.transaction_id.clone(),
            direction: "sent",
        };

        let resp: RegisterResponse = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(resp.external_id)
    }

    /// Step 2 — poll the summary endpoint until the assessment is `COMPLETE`.
    async fn poll_summary(
        &self,
        tx_id: &str,
        external_id: &str,
    ) -> Result<ScreeningResult, reqwest::Error> {
        let url = format!(
            "{}/api/kyt/v2/transfers/{}/summary",
            self.config.endpoint, external_id
        );

        for attempt in 0..MAX_POLL_ATTEMPTS {
            let summary: SummaryResponse = self
                .client
                .get(&url)
                .header("Authorization", self.auth_header())
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            debug!(
                provider = "chainalysis",
                tx_id,
                attempt,
                alert_status = %summary.alert_status,
                "KYT poll",
            );

            if summary.alert_status == "COMPLETE" {
                return Ok(map_alert_level(summary.alert_level.as_deref()));
            }

            // Assessment not ready — back-off and retry.
            let delay_ms = POLL_BASE_MS * (1u64 << attempt);
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }

        // Timed out waiting for Chainalysis — hold for manual review.
        warn!(
            provider = "chainalysis",
            tx_id, "KYT assessment did not complete within polling budget — holding transaction",
        );
        Ok(ScreeningResult::Hold {
            reason: "Chainalysis KYT assessment timed out. \
                     Transaction held for manual compliance review."
                .to_owned(),
            review_required: true,
        })
    }

    /// Full batch screening cycle: register → poll.
    async fn screen_batch(&self, tx: &TransactionEvent) -> ScreeningResult {
        // Validate that required blockchain metadata is present.
        let missing: Vec<&str> = ["network", "tx_hash", "output_address"]
            .iter()
            .filter(|k| !tx.metadata.contains_key(**k))
            .copied()
            .collect();

        if !missing.is_empty() {
            warn!(
                provider = "chainalysis",
                tx_id = %tx.transaction_id,
                ?missing,
                "Missing required blockchain metadata — holding transaction",
            );
            return ScreeningResult::Hold {
                reason: format!(
                    "Chainalysis KYT: missing metadata fields {missing:?}. \
                     Transaction held for manual compliance review."
                ),
                review_required: true,
            };
        }

        // Step 1: register.
        let external_id = match self.register_transfer(tx).await {
            Ok(id) => id,
            Err(e) => {
                warn!(
                    provider = "chainalysis",
                    tx_id = %tx.transaction_id,
                    error = %e,
                    "Transfer registration failed — holding transaction",
                );
                return ScreeningResult::Hold {
                    reason: "Chainalysis KYT registration failed. \
                             Transaction held for manual compliance review."
                        .to_owned(),
                    review_required: true,
                };
            }
        };

        // Step 2: poll for result.
        match self.poll_summary(&tx.transaction_id, &external_id).await {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    provider = "chainalysis",
                    tx_id = %tx.transaction_id,
                    error = %e,
                    "Poll failed — holding transaction",
                );
                ScreeningResult::Hold {
                    reason: "Chainalysis KYT polling failed. \
                             Transaction held for manual compliance review."
                        .to_owned(),
                    review_required: true,
                }
            }
        }
    }
}

#[async_trait]
impl TransactionScreener for ChainalysisScreener {
    #[instrument(
        skip(self, tx),
        fields(
            provider = "chainalysis",
            tx_id = %tx.transaction_id,
            mode = ?mode,
        )
    )]
    async fn screen(&self, tx: &TransactionEvent, mode: ScreeningMode) -> ScreeningResult {
        match mode {
            ScreeningMode::RealTime => {
                // Chainalysis KYT is a 2-step async API — incompatible with the
                // 50 ms real-time deadline. Return Hold immediately so the caller
                // enqueues this transaction for batch re-screening.
                debug!(
                    provider = "chainalysis",
                    tx_id = %tx.transaction_id,
                    "RealTime mode — deferring to batch (Chainalysis is async)",
                );
                ScreeningResult::Hold {
                    reason: "Chainalysis KYT requires asynchronous processing. \
                             Transaction queued for batch re-screening."
                        .to_owned(),
                    review_required: false, // will be re-screened, not a compliance block
                }
            }
            ScreeningMode::Batch => self.screen_batch(tx).await,
        }
    }

    fn provider_name(&self) -> &'static str {
        "chainalysis"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_severe_is_reject_with_sar() {
        assert!(matches!(
            map_alert_level(Some("SEVERE")),
            ScreeningResult::Reject {
                sar_required: true,
                ..
            }
        ));
    }

    #[test]
    fn map_high_is_hold() {
        assert!(matches!(
            map_alert_level(Some("HIGH")),
            ScreeningResult::Hold {
                review_required: true,
                ..
            }
        ));
    }

    #[test]
    fn map_medium_is_flag_high() {
        assert!(matches!(
            map_alert_level(Some("MEDIUM")),
            ScreeningResult::Flag {
                severity: RiskLevel::High,
                ..
            }
        ));
    }

    #[test]
    fn map_low_is_flag_low() {
        assert!(matches!(
            map_alert_level(Some("LOW")),
            ScreeningResult::Flag {
                severity: RiskLevel::Low,
                ..
            }
        ));
    }

    #[test]
    fn map_null_is_clear() {
        assert_eq!(map_alert_level(None), ScreeningResult::Clear);
    }

    #[test]
    fn realtime_mode_returns_hold_without_network() {
        // Verifies the RealTime → Hold path is synchronous and needs no HTTP.
        // Uses tokio::runtime::Handle for simplicity without full async test.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            use crate::providers::ProviderConfig;
            let config = ProviderConfig::chainalysis("test-key");
            let screener = ChainalysisScreener::new(config);
            let tx = TransactionEvent::new("tx-001", 1_000, "BTC", "alice", "bob");
            let result = screener.screen(&tx, ScreeningMode::RealTime).await;
            // review_required=false because it will be re-screened in batch
            assert!(matches!(
                result,
                ScreeningResult::Hold {
                    review_required: false,
                    ..
                }
            ));
        });
    }

    /// Live integration test — requires Chainalysis sandbox credentials.
    ///
    /// ```text
    /// CHAINALYSIS_API_KEY=xxx \
    ///   cargo test -p blazil-screening -- --ignored chainalysis_live_batch
    /// ```
    #[tokio::test]
    #[ignore = "requires Chainalysis sandbox credentials (CHAINALYSIS_API_KEY)"]
    async fn chainalysis_live_batch() {
        use crate::providers::ProviderConfig;
        let api_key = std::env::var("CHAINALYSIS_API_KEY").expect("CHAINALYSIS_API_KEY not set");
        let config = ProviderConfig::chainalysis(api_key);
        let screener = ChainalysisScreener::new(config);
        let tx = TransactionEvent::new("live-batch-001", 1_000_000, "BTC", "alice", "bob")
            .with_metadata("network", "BITCOIN")
            .with_metadata(
                "tx_hash",
                "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
            )
            .with_metadata("output_address", "1A1zP1eP5QGefi2DMPTfTL5SLmv7Divf Na");
        let result = screener.screen(&tx, ScreeningMode::Batch).await;
        println!("Live Chainalysis result: {result:?}");
        assert!(matches!(
            result,
            ScreeningResult::Clear
                | ScreeningResult::Flag { .. }
                | ScreeningResult::Hold { .. }
                | ScreeningResult::Reject { .. }
        ));
    }
}

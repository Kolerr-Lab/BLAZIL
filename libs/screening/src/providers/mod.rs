// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! External screening provider integration layer.
//!
//! Each provider module contains:
//! - A `*Screener` struct that implements `TransactionScreener`.
//! - A region-aware factory constructor via [`ProviderConfig`].
//! - `build_request` / `parse_response` private helpers — the only functions
//!   that need updating when a provider changes its API schema.
//!
//! # Adding a new provider
//!
//! 1. Create `providers/{name}.rs` with a `{Name}Screener` struct.
//! 2. Implement `TransactionScreener` (use `SardineScreener` as the template).
//! 3. Add a `ProviderConfig::{name}()` factory method to this file.
//! 4. Add `pub mod {name};` below.
//! 5. Write unit tests for the score-mapping logic.
//! 6. Write `#[ignore]` integration tests (activated once sandbox creds exist).
//!
//! # HTTP infrastructure
//!
//! All providers share the same patterns:
//! - `reqwest::Client` is built **once** at construction time and reused
//!   (it manages a connection pool internally).
//! - TLS via `rustls` — no native-tls dependency.
//! - `config.timeout` is applied per-request via `Client::builder`.
//! - Retry (up to `config.max_retries`) is **only** applied in `Batch` mode.
//!   Real-time calls make a single attempt to stay within the 50 ms deadline.
//!
//! # Secrets
//!
//! API keys **must** be sourced from environment variables injected by the
//! Vault-backed secrets service at startup (see `libs/secrets/`).
//! Hard-coding or logging credentials is strictly forbidden.

use std::time::Duration;

pub mod chainalysis;
pub mod elliptic;
pub mod sardine;

// ── Region ────────────────────────────────────────────────────────────────────

/// Deployment region — controls the regulatory context signalled to providers
/// that support multi-jurisdiction rule sets (Sardine, Elliptic) and the
/// endpoint URL for providers with regional nodes.
///
/// | Variant          | Regulatory context                    |
/// |------------------|---------------------------------------|
/// | `Apac`           | MAS TRM — Singapore / ANZ             |
/// | `NorthAmerica`   | FinCEN / FINTRAC — US / Canada        |
/// | `Eu`             | AMLD6 / DORA — European Union         |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Singapore / APAC — Monetary Authority of Singapore TRM context.
    Apac,
    /// United States / Canada — FinCEN and FINTRAC context.
    NorthAmerica,
    /// European Union — AMLD6 / DORA context.
    Eu,
}

impl Region {
    /// Short uppercase tag forwarded to providers for rule-set selection.
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Apac => "APAC",
            Self::NorthAmerica => "NA",
            Self::Eu => "EU",
        }
    }
}

// ── ProviderConfig ────────────────────────────────────────────────────────────

/// Configuration for an HTTP-based screening provider.
///
/// Build instances via the provider-specific factory methods
/// ([`ProviderConfig::sardine`], [`ProviderConfig::elliptic`],
/// [`ProviderConfig::chainalysis`]) rather than the struct literal — factories
/// hard-code the correct endpoint URLs and validate required credential fields.
///
/// # Security
///
/// Load all credential fields from environment variables at startup via the
/// Vault-backed secrets service. Never log `api_key`, `api_secret`, or
/// `client_id` values.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Base URL of the provider's screening API (no trailing slash).
    pub endpoint: String,

    /// Primary API key / token. Meaning is provider-specific:
    /// - Sardine: `Sardine-Secret-Key` header value
    /// - Elliptic: `x-access-key` header value
    /// - Chainalysis: `Token` credential
    ///
    /// Load from secrets manager at startup; do not log or embed this value.
    pub api_key: String,

    /// HMAC signing secret — required for Elliptic's `x-access-sign` scheme.
    /// `None` for providers that do not use request signing.
    pub api_secret: Option<String>,

    /// Client/tenant identifier — required for Sardine's `Sardine-Client-Id`
    /// header. `None` for providers that use a single credential.
    pub client_id: Option<String>,

    /// Deployment region. Forwarded to providers that support multi-region
    /// routing or jurisdiction-specific rule sets.
    pub region: Region,

    /// Per-request timeout. Default: 45 ms — leaves 5 ms headroom under the
    /// 50 ms real-time deadline for network overhead.
    /// Chainalysis (batch-only) uses a 5 s default.
    pub timeout: Duration,

    /// Maximum retry attempts on transient errors (5xx, connection reset).
    /// Retries use exponential back-off with full jitter.
    /// **Retries are only applied in `Batch` mode** — real-time calls make a
    /// single attempt to avoid breaching the 50 ms deadline.
    pub max_retries: u32,
}

impl ProviderConfig {
    // ── Sardine ───────────────────────────────────────────────────────────────

    /// Creates a Sardine AI config for the given region.
    ///
    /// Sardine operates a single global endpoint; the region tag is forwarded
    /// in the request body for jurisdiction-specific rule-set selection.
    ///
    /// # Credentials
    ///
    /// Load from environment variables injected by the secrets service:
    /// ```text
    /// SARDINE_CLIENT_ID   → client_id
    /// SARDINE_SECRET_KEY  → api_key  (Sardine-Secret-Key header)
    /// ```
    pub fn sardine(
        region: Region,
        client_id: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: "https://api.sardine.ai".to_owned(),
            api_key: secret_key.into(),
            api_secret: None,
            client_id: Some(client_id.into()),
            region,
            timeout: Duration::from_millis(45),
            max_retries: 2,
        }
    }

    // ── Elliptic ──────────────────────────────────────────────────────────────

    /// Creates an Elliptic AML config for the given region.
    ///
    /// Elliptic operates a single global endpoint; the region is forwarded in
    /// the request body for jurisdiction selection.
    ///
    /// Both credential fields are required — Elliptic uses HMAC-SHA256 request
    /// signing in addition to the API key.
    ///
    /// # Credentials
    ///
    /// ```text
    /// ELLIPTIC_API_KEY    → api_key    (x-access-key header)
    /// ELLIPTIC_API_SECRET → api_secret (HMAC-SHA256 signing secret)
    /// ```
    pub fn elliptic(
        region: Region,
        api_key: impl Into<String>,
        api_secret: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: "https://aml-api.elliptic.co".to_owned(),
            api_key: api_key.into(),
            api_secret: Some(api_secret.into()),
            client_id: None,
            region,
            timeout: Duration::from_millis(45),
            max_retries: 2,
        }
    }

    // ── Chainalysis ───────────────────────────────────────────────────────────

    /// Creates a Chainalysis KYT config.
    ///
    /// Chainalysis uses a 2-step register-then-poll flow and is therefore
    /// **Batch-only**; real-time calls return `Hold` immediately without an
    /// API round-trip.
    ///
    /// Timeout is set to 5 s (batch path only; real-time path never calls
    /// the network).
    ///
    /// # Credentials
    ///
    /// ```text
    /// CHAINALYSIS_API_KEY → api_key  (Token header)
    /// ```
    pub fn chainalysis(api_key: impl Into<String>) -> Self {
        Self {
            endpoint: "https://api.chainalysis.com".to_owned(),
            api_key: api_key.into(),
            api_secret: None,
            client_id: None,
            region: Region::NorthAmerica, // Chainalysis has no regional routing
            timeout: Duration::from_secs(5),
            max_retries: 2,
        }
    }

    // ── Builder helpers ───────────────────────────────────────────────────────

    /// Overrides the per-request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Overrides the maximum retry count.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

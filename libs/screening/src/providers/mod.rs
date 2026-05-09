// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! External screening provider integration layer.
//!
//! Each provider module contains:
//! - A `*Screener` struct that implements `TransactionScreener`.
//! - A `ProviderConfig` that carries endpoint, auth, timeout, and retry config.
//! - A wire-ready skeleton: `build_request` / `parse_response` are the only
//!   methods that need to be filled in once the API contract is signed.
//!
//! All HTTP infrastructure (auth header injection, timeout, exponential-backoff
//! retry) is specified in `ProviderConfig` and will be centralised in the
//! `HttpScreenerClient` base once the first provider goes live.

use std::time::Duration;

pub mod chainalysis;
pub mod elliptic;
pub mod sardine;

/// Configuration shared by all HTTP-based screening providers.
///
/// API keys **must** be sourced from the secrets manager (e.g. AWS Secrets
/// Manager via `blazil-secrets`). Hard-coding credentials is forbidden.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Base URL of the provider's screening API.
    pub endpoint: String,

    /// API key for authentication. Load from secrets manager at startup;
    /// do not log or embed this value.
    pub api_key: String,

    /// Per-request timeout. Default is 45 ms — leaving 5 ms headroom under
    /// the 50 ms real-time deadline for network overhead.
    pub timeout: Duration,

    /// Maximum retry attempts on transient errors (5xx, connection reset).
    /// Retries use exponential backoff with jitter.
    pub max_retries: u32,
}

impl ProviderConfig {
    /// Creates a config with default timeout (45 ms) and retry count (2).
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            timeout: Duration::from_millis(45),
            max_retries: 2,
        }
    }

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

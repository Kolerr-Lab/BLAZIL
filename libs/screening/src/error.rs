// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Error types for the screening crate.

use thiserror::Error;

/// Errors that can occur during compliance screening operations.
#[derive(Debug, Error)]
pub enum ScreeningError {
    /// An external provider returned an unexpected or malformed response.
    #[error("provider '{provider}' returned invalid response: {detail}")]
    InvalidProviderResponse {
        provider: &'static str,
        detail: String,
    },

    /// Network or I/O failure communicating with an external provider.
    #[error("provider '{provider}' communication error: {source}")]
    ProviderCommunication {
        provider: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The hold store failed to create or update a hold record.
    #[error("hold store error: {0}")]
    HoldStore(String),

    /// SAR XML serialization failed.
    #[error("SAR serialization failed: {0}")]
    SarSerialization(String),

    /// The batch worker channel is closed; the worker task has exited.
    #[error("batch worker channel closed")]
    BatchChannelClosed,
}

//! Aeron IPC inference protocol using MessagePack serialization.
//!
//! ## Wire Protocol
//!
//! ```text
//! Client → Aeron:IPC (stream 2001) → InferenceServer
//!   → Model inference → Result
//!   → Aeron:IPC (stream 2002) → Client
//! ```
//!
//! ## Message Format
//!
//! MessagePack binary encoding:
//! - Compact: 30-50% smaller than JSON
//! - Fast: no text parsing overhead
//! - Type-safe: Serde-compatible
//!
//! ## Error Handling
//!
//! Errors are returned in `InferenceResponse.error` field.
//! Empty error string means success.

use serde::{Deserialize, Serialize};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Stream ID for inbound client→server inference requests.
pub const INFERENCE_REQ_STREAM_ID: i32 = 2001;

/// Stream ID for outbound server→client inference responses.
pub const INFERENCE_RSP_STREAM_ID: i32 = 2002;

// ── InferenceRequest ──────────────────────────────────────────────────────────

/// Inference request sent by a client over Aeron IPC.
///
/// # Examples
///
/// ```rust
/// use blazil_inference_service::protocol::{InferenceRequest, serialize_request};
///
/// let req = InferenceRequest {
///     request_id: "req-001".to_string(),
///     input_data: vec![0u8; 224 * 224 * 3], // 224x224 RGB image
///     input_shape: vec![224, 224, 3],
///     model_version: "v1".to_string(),
/// };
/// let bytes = serialize_request(&req).unwrap();
/// assert!(bytes.len() < req.input_data.len() + 100); // MessagePack overhead minimal
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    /// Client-generated request ID for correlation.
    ///
    /// Used to match async responses with requests in pipelined mode.
    pub request_id: String,

    /// Raw input data (e.g., image bytes, embeddings).
    ///
    /// Format depends on model: typically flattened HWC or CHW bytes.
    pub input_data: Vec<u8>,

    /// Input tensor shape: [height, width, channels] or [batch, channels, height, width].
    ///
    /// Server validates this matches model expectations.
    pub input_shape: Vec<u32>,

    /// Model version identifier (optional, defaults to "latest").
    ///
    /// Future: support multiple model versions in same server.
    pub model_version: String,
}

// ── InferenceResponse ─────────────────────────────────────────────────────────

/// Inference response sent back to the client.
///
/// # Examples
///
/// ```rust
/// use blazil_inference_service::protocol::{InferenceResponse, deserialize_response};
///
/// let resp = InferenceResponse {
///     request_id: "req-001".to_string(),
///     class_id: Some(281),
///     probabilities: vec![0.001, 0.002, 0.95],
///     raw_output: vec![],
///     confidence: 0.95,
///     latency_us: 1523,
///     error: String::new(),
/// };
/// let bytes = rmp_serde::to_vec(&resp).unwrap();
/// let decoded: InferenceResponse = deserialize_response(&bytes).unwrap();
/// assert_eq!(decoded.request_id, resp.request_id);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    /// Request ID from the original request.
    pub request_id: String,

    /// Predicted class ID (classification models).
    ///
    /// `None` for regression or when error occurred.
    pub class_id: Option<u32>,

    /// Class probabilities (classification models).
    ///
    /// Length = num_classes. Empty for regression or errors.
    pub probabilities: Vec<f32>,

    /// Raw model output (regression models or when full output needed).
    ///
    /// Empty for classification unless explicitly requested.
    pub raw_output: Vec<f32>,

    /// Confidence score [0.0, 1.0].
    ///
    /// For classification: max probability.
    /// For regression: 0.0 (not applicable).
    pub confidence: f32,

    /// End-to-end latency in microseconds.
    ///
    /// Measured from request receipt to response send.
    pub latency_us: u64,

    /// Error message (empty string = success).
    ///
    /// Non-empty means inference failed. Client should log and retry.
    pub error: String,
}

// ── Serialization Helpers ─────────────────────────────────────────────────────

/// Serialize an `InferenceRequest` to MessagePack bytes.
#[allow(dead_code)]
pub fn serialize_request(req: &InferenceRequest) -> anyhow::Result<Vec<u8>> {
    rmp_serde::to_vec(req).map_err(|e| anyhow::anyhow!("serialize request: {e}"))
}

/// Deserialize an `InferenceRequest` from MessagePack bytes.
pub fn deserialize_request(data: &[u8]) -> anyhow::Result<InferenceRequest> {
    rmp_serde::from_slice(data).map_err(|e| anyhow::anyhow!("deserialize request: {e}"))
}

/// Serialize an `InferenceResponse` to MessagePack bytes.
pub fn serialize_response(resp: &InferenceResponse) -> anyhow::Result<Vec<u8>> {
    rmp_serde::to_vec(resp).map_err(|e| anyhow::anyhow!("serialize response: {e}"))
}

/// Deserialize an `InferenceResponse` from MessagePack bytes.
#[allow(dead_code)]
pub fn deserialize_response(data: &[u8]) -> anyhow::Result<InferenceResponse> {
    rmp_serde::from_slice(data).map_err(|e| anyhow::anyhow!("deserialize response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_roundtrip() {
        let req = InferenceRequest {
            request_id: "test-123".to_string(),
            input_data: vec![42u8; 1000],
            input_shape: vec![10, 10, 10],
            model_version: "v1.0".to_string(),
        };

        let bytes = serialize_request(&req).unwrap();
        let decoded = deserialize_request(&bytes).unwrap();

        assert_eq!(decoded.request_id, req.request_id);
        assert_eq!(decoded.input_data, req.input_data);
        assert_eq!(decoded.input_shape, req.input_shape);
        assert_eq!(decoded.model_version, req.model_version);
    }

    #[test]
    fn test_response_roundtrip() {
        let resp = InferenceResponse {
            request_id: "test-456".to_string(),
            class_id: Some(123),
            probabilities: vec![0.1, 0.2, 0.7],
            raw_output: vec![],
            confidence: 0.7,
            latency_us: 1500,
            error: String::new(),
        };

        let bytes = serialize_response(&resp).unwrap();
        let decoded = deserialize_response(&bytes).unwrap();

        assert_eq!(decoded.request_id, resp.request_id);
        assert_eq!(decoded.class_id, resp.class_id);
        assert_eq!(decoded.probabilities, resp.probabilities);
        assert_eq!(decoded.confidence, resp.confidence);
        assert_eq!(decoded.latency_us, resp.latency_us);
    }

    #[test]
    fn test_error_response() {
        let resp = InferenceResponse {
            request_id: "err-789".to_string(),
            class_id: None,
            probabilities: vec![],
            raw_output: vec![],
            confidence: 0.0,
            latency_us: 100,
            error: "Model not found".to_string(),
        };

        let bytes = serialize_response(&resp).unwrap();
        let decoded = deserialize_response(&bytes).unwrap();

        assert_eq!(decoded.error, "Model not found");
        assert!(decoded.class_id.is_none());
    }
}

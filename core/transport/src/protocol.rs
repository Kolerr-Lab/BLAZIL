//! Wire protocol: frames, messages, and serialization.
//!
//! Blazil uses a simple **length-prefixed binary protocol**:
//!
//! ```text
//! ┌──────────────────┬────────────────────────┐
//! │  4 bytes (u32 BE)│  N bytes (MessagePack) │
//! │  frame length    │  serialized payload    │
//! └──────────────────┴────────────────────────┘
//! ```
//!
//! ## Why MessagePack?
//!
//! - Binary encoding: 30–50% smaller than JSON
//! - No text parsing overhead
//! - Serde-compatible via `rmp-serde`
//! - Battle-tested in financial messaging
//! - Upgradeable to SBE in a future prompt
//!
//! ## Frame size limit
//!
//! Frames larger than [`MAX_FRAME_SIZE`] (1 MiB) are rejected and the
//! connection is closed. This prevents memory exhaustion from malicious or
//! malformed clients.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use blazil_common::error::{BlazerError, BlazerResult};

/// Maximum frame payload size: 1 MiB.
pub const MAX_FRAME_SIZE: usize = 1_048_576;

// ── TransactionRequest ────────────────────────────────────────────────────────

/// A transaction request sent by a client over the wire.
///
/// All fields are plain strings or integers to keep the protocol
/// independent of Blazil's internal domain types.
///
/// # Examples
///
/// ```rust
/// use blazil_transport::protocol::{TransactionRequest, serialize_request, deserialize_request};
///
/// let req = TransactionRequest {
///     request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
///     debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
///     credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
///     amount: "100.00".into(),
///     currency: "USD".into(),
///     ledger_id: 1,
///     code: 1,
/// };
/// let bytes = serialize_request(&req).unwrap();
/// let decoded = deserialize_request(&bytes).unwrap();
/// assert_eq!(decoded.request_id, req.request_id);
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransactionRequest {
    /// Client-generated idempotency key.
    ///
    /// Same `request_id` = same transaction = safe to retry.
    /// Should be a UUID v4 string. Non-UUID values are accepted but a new
    /// `TransactionId` is generated and the client is warned.
    pub request_id: String,

    /// Debit account ID (UUID string). Money leaves this account.
    pub debit_account_id: String,

    /// Credit account ID (UUID string). Money arrives at this account.
    pub credit_account_id: String,

    /// Amount as a decimal string to preserve precision.
    ///
    /// Format: `"100.00"` — must be a positive decimal with at most 8
    /// decimal places.
    pub amount: String,

    /// ISO 4217 currency code, e.g. `"USD"`, `"VND"`.
    pub currency: String,

    /// Ledger ID (u32). Maps to a [`blazil_common::ids::LedgerId`] constant.
    pub ledger_id: u32,

    /// Application-level transaction type code.
    pub code: u16,
}

// ── TransactionResponse ───────────────────────────────────────────────────────

/// A response sent back to the client after processing a transaction.
///
/// # Examples
///
/// ```rust
/// use blazil_transport::protocol::{TransactionResponse, serialize_response, deserialize_response};
///
/// let resp = TransactionResponse {
///     request_id: "req-001".into(),
///     committed: true,
///     transfer_id: Some("550e8400-e29b-41d4-a716-446655440010".into()),
///     error: None,
///     timestamp_ns: 1_000_000_000,
/// };
/// let bytes = serialize_response(&resp).unwrap();
/// let decoded = deserialize_response(&bytes).unwrap();
/// assert_eq!(decoded.committed, true);
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransactionResponse {
    /// Echoed from the request for client-side correlation.
    pub request_id: String,

    /// `true` if the transaction was committed; `false` if rejected.
    pub committed: bool,

    /// Transfer ID assigned by the ledger if committed (UUID string).
    pub transfer_id: Option<String>,

    /// Human-readable error message if rejected.
    pub error: Option<String>,

    /// Server-side processing completion timestamp in nanoseconds since epoch.
    pub timestamp_ns: u64,
}

// ── Frame ─────────────────────────────────────────────────────────────────────

/// A length-prefixed binary frame read from the wire.
///
/// The frame header is a 4-byte big-endian `u32` indicating the payload
/// length. A frame with an empty `payload` is valid (e.g. a ping).
///
/// # Examples
///
/// ```rust
/// use blazil_transport::protocol::Frame;
///
/// let payload = b"hello, blazil";
/// let wire_bytes = Frame::encode(payload);
/// // first 4 bytes are the length
/// let len = u32::from_be_bytes(wire_bytes[..4].try_into().unwrap()) as usize;
/// assert_eq!(len, payload.len());
/// assert_eq!(&wire_bytes[4..], payload);
/// ```
#[derive(Debug)]
pub struct Frame {
    /// The raw MessagePack payload bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Encodes a payload into a length-prefixed wire frame.
    ///
    /// Returns `[4-byte BE length | payload bytes]`.
    pub fn encode(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u32;
        let mut buf = Vec::with_capacity(4 + payload.len());
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    /// Reads one frame from a [`TcpStream`].
    ///
    /// Reads the 4-byte length header first, then reads exactly that many
    /// bytes of payload.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::Transport`] on EOF or I/O error.
    /// - [`BlazerError::Transport`] if the frame exceeds [`MAX_FRAME_SIZE`].
    pub async fn read_frame(stream: &mut TcpStream) -> BlazerResult<Self> {
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| BlazerError::Transport(format!("failed to read frame header: {e}")))?;

        let len = u32::from_be_bytes(len_buf) as usize;

        if len > MAX_FRAME_SIZE {
            return Err(BlazerError::Transport(format!(
                "frame size {len} exceeds maximum {MAX_FRAME_SIZE}"
            )));
        }

        let mut payload = vec![0u8; len];
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| BlazerError::Transport(format!("failed to read frame payload: {e}")))?;

        Ok(Self { payload })
    }

    /// Writes a length-prefixed frame to a [`TcpStream`].
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] on any I/O error.
    pub async fn write_frame(stream: &mut TcpStream, payload: &[u8]) -> BlazerResult<()> {
        let wire = Self::encode(payload);
        stream
            .write_all(&wire)
            .await
            .map_err(|e| BlazerError::Transport(format!("failed to write frame: {e}")))?;
        Ok(())
    }
}

// ── Serialization helpers ─────────────────────────────────────────────────────

/// Serializes a [`TransactionRequest`] to MessagePack bytes.
///
/// # Errors
///
/// Returns [`BlazerError::Transport`] if serialization fails.
pub fn serialize_request(req: &TransactionRequest) -> BlazerResult<Vec<u8>> {
    rmp_serde::to_vec(req)
        .map_err(|e| BlazerError::Transport(format!("request serialization failed: {e}")))
}

/// Deserializes a [`TransactionRequest`] from MessagePack bytes.
///
/// # Errors
///
/// Returns [`BlazerError::Transport`] if the bytes are malformed.
pub fn deserialize_request(bytes: &[u8]) -> BlazerResult<TransactionRequest> {
    rmp_serde::from_slice(bytes)
        .map_err(|e| BlazerError::Transport(format!("request deserialization failed: {e}")))
}

/// Serializes a [`TransactionResponse`] to MessagePack bytes.
///
/// # Errors
///
/// Returns [`BlazerError::Transport`] if serialization fails.
pub fn serialize_response(resp: &TransactionResponse) -> BlazerResult<Vec<u8>> {
    rmp_serde::to_vec(resp)
        .map_err(|e| BlazerError::Transport(format!("response serialization failed: {e}")))
}

/// Deserializes a [`TransactionResponse`] from MessagePack bytes.
///
/// # Errors
///
/// Returns [`BlazerError::Transport`] if the bytes are malformed.
pub fn deserialize_response(bytes: &[u8]) -> BlazerResult<TransactionResponse> {
    rmp_serde::from_slice(bytes)
        .map_err(|e| BlazerError::Transport(format!("response deserialization failed: {e}")))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> TransactionRequest {
        TransactionRequest {
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
            credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
            amount: "100.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
        }
    }

    fn sample_response() -> TransactionResponse {
        TransactionResponse {
            request_id: "req-001".into(),
            committed: true,
            transfer_id: Some("550e8400-e29b-41d4-a716-446655440010".into()),
            error: None,
            timestamp_ns: 1_000_000_000,
        }
    }

    // ── Frame tests ───────────────────────────────────────────────────────────

    #[test]
    fn frame_encode_prepends_length_header() {
        let payload = b"hello blazil";
        let wire = Frame::encode(payload);
        assert_eq!(wire.len(), 4 + payload.len());
        let len = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(len, payload.len());
        assert_eq!(&wire[4..], payload);
    }

    #[test]
    fn frame_encode_empty_payload() {
        let wire = Frame::encode(b"");
        assert_eq!(wire.len(), 4);
        let len = u32::from_be_bytes(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(len, 0);
    }

    #[tokio::test]
    async fn frame_read_write_round_trip() {
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let payload = b"round-trip frame payload";
        let payload_clone = payload.to_vec();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let frame = Frame::read_frame(&mut stream).await.unwrap();
            frame.payload
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        Frame::write_frame(&mut client, payload).await.unwrap();

        let received = server.await.unwrap();
        assert_eq!(received, payload_clone);
    }

    #[tokio::test]
    async fn read_frame_rejects_oversized_frame() {
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            Frame::read_frame(&mut stream).await
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        // Write a header claiming 2 MiB (> MAX_FRAME_SIZE)
        let oversized_len = (MAX_FRAME_SIZE + 1) as u32;
        client
            .write_all(&oversized_len.to_be_bytes())
            .await
            .unwrap();

        let result = server.await.unwrap();
        assert!(
            result.is_err(),
            "expected Transport error for oversized frame"
        );
        assert!(matches!(result.unwrap_err(), BlazerError::Transport(_)));
    }

    // ── Serde round-trip tests ────────────────────────────────────────────────

    #[test]
    fn transaction_request_serde_round_trip() {
        let original = sample_request();
        let bytes = serialize_request(&original).unwrap();
        let decoded = deserialize_request(&bytes).unwrap();

        assert_eq!(decoded.request_id, original.request_id);
        assert_eq!(decoded.debit_account_id, original.debit_account_id);
        assert_eq!(decoded.credit_account_id, original.credit_account_id);
        assert_eq!(decoded.amount, original.amount);
        assert_eq!(decoded.currency, original.currency);
        assert_eq!(decoded.ledger_id, original.ledger_id);
        assert_eq!(decoded.code, original.code);
    }

    #[test]
    fn transaction_response_serde_round_trip() {
        let original = sample_response();
        let bytes = serialize_response(&original).unwrap();
        let decoded = deserialize_response(&bytes).unwrap();

        assert_eq!(decoded.request_id, original.request_id);
        assert_eq!(decoded.committed, original.committed);
        assert_eq!(decoded.transfer_id, original.transfer_id);
        assert_eq!(decoded.error, original.error);
        assert_eq!(decoded.timestamp_ns, original.timestamp_ns);
    }

    #[test]
    fn response_with_rejection_round_trips() {
        let original = TransactionResponse {
            request_id: "req-002".into(),
            committed: false,
            transfer_id: None,
            error: Some("self-transfer not allowed".into()),
            timestamp_ns: 42,
        };
        let bytes = serialize_response(&original).unwrap();
        let decoded = deserialize_response(&bytes).unwrap();

        assert!(!decoded.committed);
        assert!(decoded.transfer_id.is_none());
        assert_eq!(decoded.error.as_deref(), Some("self-transfer not allowed"));
    }

    #[test]
    fn malformed_bytes_return_transport_error() {
        let garbage = b"\xc1\xc1\xc1\xc1gibberish";
        let result = deserialize_request(garbage);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BlazerError::Transport(_)));
    }
}

//! AF_XDP bench client — sends BLZL-framed UDP requests to an
//! [`AfXdpTransportServer`] and receives MessagePack responses.
//!
//! # Wire protocol (client → server)
//!
//! ```text
//! ┌──────────────────────┬──────────────────────────────────────┐
//! │  BLZL_MAGIC (4 B)    │  MsgPack(TransactionRequest)         │
//! └──────────────────────┴──────────────────────────────────────┘
//! ```
//!
//! The UDP payload is intercepted by the XDP filter at the server NIC and
//! redirected directly into the AF_XDP socket ring buffer (zero kernel path).
//!
//! # Wire protocol (server → client)
//!
//! Plain `MsgPack(TransactionResponse)` UDP datagram.
//! The server uses a standard `std::net::UdpSocket` for responses.
//!
//! # Usage
//!
//! ```rust,no_run
//! # #[cfg(all(target_os = "linux", feature = "af-xdp"))]
//! # fn main() -> blazil_common::error::BlazerResult<()> {
//! use std::time::Duration;
//! use blazil_transport::afxdp::client::AfXdpClient;
//! use blazil_transport::protocol::TransactionRequest;
//!
//! let client = AfXdpClient::from_env()?;
//! let req = TransactionRequest {
//!     request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
//!     debit_account_id:  "account-debit".into(),
//!     credit_account_id: "account-credit".into(),
//!     amount: "100.00".into(),
//!     currency: "USD".into(),
//!     ledger_id: 1,
//!     code: 1,
//! };
//! let (resp, rtt) = client.roundtrip(&req, Duration::from_millis(200))?;
//! assert!(resp.committed || resp.error.is_some());
//! # Ok(())
//! # }
//! ```

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use blazil_common::error::{BlazerError, BlazerResult};

use crate::protocol::{
    deserialize_response, encode_blzl_frame, TransactionRequest, TransactionResponse, BLZL_UDP_PORT,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum UDP datagram buffer size (65_535 is the UDP payload limit).
const MAX_UDP_PAYLOAD: usize = 65_535;

// ── AfXdpClient ───────────────────────────────────────────────────────────────

/// UDP client for sending BLZL-framed requests to an [`AfXdpTransportServer`].
///
/// Each instance owns one bound UDP socket.  The server echoes the
/// `request_id` in the response, allowing the caller to correlate replies
/// when using windowed (pipelined) operation.
///
/// # Thread safety
///
/// `AfXdpClient` is `Send` but **not** `Sync`.  Each bench shard/thread
/// should create its own instance.
pub struct AfXdpClient {
    socket: UdpSocket,
    server_addr: SocketAddr,
}

impl AfXdpClient {
    /// Create a client that sends to `server_addr`.
    ///
    /// Binds to an ephemeral port on `0.0.0.0`.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the socket cannot be bound.
    pub fn connect(server_addr: SocketAddr) -> BlazerResult<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| BlazerError::Transport(format!("AfXdpClient bind failed: {e}")))?;
        // Default receive timeout — callers can override via set_read_timeout.
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| BlazerError::Transport(format!("set_read_timeout: {e}")))?;
        Ok(Self {
            socket,
            server_addr,
        })
    }

    /// Build a client from the `BLAZIL_XDP_SERVER_ADDR` environment variable.
    ///
    /// | Variable                  | Default             |
    /// |---------------------------|---------------------|
    /// | `BLAZIL_XDP_SERVER_ADDR`  | `"127.0.0.1:7878"`  |
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the address is unparseable or
    /// the socket bind fails.
    pub fn from_env() -> BlazerResult<Self> {
        let addr_str = std::env::var("BLAZIL_XDP_SERVER_ADDR")
            .unwrap_or_else(|_| format!("127.0.0.1:{BLZL_UDP_PORT}"));
        let server_addr: SocketAddr = addr_str.parse().map_err(|e| {
            BlazerError::Transport(format!(
                "BLAZIL_XDP_SERVER_ADDR '{addr_str}' is not a valid SocketAddr: {e}"
            ))
        })?;
        Self::connect(server_addr)
    }

    /// Send a BLZL-framed UDP datagram to the server.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] on I/O error.
    pub fn send_request(&self, frame: &[u8]) -> BlazerResult<()> {
        self.socket
            .send_to(frame, self.server_addr)
            .map_err(|e| BlazerError::Transport(format!("AfXdpClient send: {e}")))?;
        Ok(())
    }

    /// Receive one UDP datagram and deserialize it as a [`TransactionResponse`].
    ///
    /// Blocks until a datagram arrives or `timeout` elapses.
    ///
    /// # Errors
    ///
    /// - [`BlazerError::Transport`] on timeout (`WouldBlock` / `TimedOut`)
    /// - [`BlazerError::Transport`] on I/O error or deserialization failure
    pub fn recv_response(&self, timeout: Duration) -> BlazerResult<TransactionResponse> {
        self.socket
            .set_read_timeout(Some(timeout))
            .map_err(|e| BlazerError::Transport(format!("set_read_timeout: {e}")))?;

        let mut buf = [0u8; MAX_UDP_PAYLOAD];
        let (n, _from) = self
            .socket
            .recv_from(&mut buf)
            .map_err(|e| BlazerError::Transport(format!("AfXdpClient recv: {e}")))?;

        deserialize_response(&buf[..n])
    }

    /// Receive one raw UDP datagram into `buf`.
    ///
    /// Returns the number of bytes written into `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] on timeout or I/O error.
    pub fn recv_raw(&self, buf: &mut [u8]) -> BlazerResult<usize> {
        let (n, _from) = self
            .socket
            .recv_from(buf)
            .map_err(|e| BlazerError::Transport(format!("AfXdpClient recv: {e}")))?;
        Ok(n)
    }

    /// Set the socket receive timeout.
    ///
    /// Used by the bench scenario to switch between blocking and non-blocking
    /// receive modes for windowed operation.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] on failure.
    pub fn set_recv_timeout(&self, timeout: Option<Duration>) -> BlazerResult<()> {
        self.socket
            .set_read_timeout(timeout)
            .map_err(|e| BlazerError::Transport(format!("set_read_timeout: {e}")))
    }

    /// Send a [`TransactionRequest`] and wait for the [`TransactionResponse`].
    ///
    /// Returns the response and the observed round-trip latency.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the send, receive, or
    /// deserialization fails.
    pub fn roundtrip(
        &self,
        req: &TransactionRequest,
        timeout: Duration,
    ) -> BlazerResult<(TransactionResponse, Duration)> {
        let frame = encode_blzl_frame(req)?;
        let t0 = Instant::now();
        self.send_request(&frame)?;
        let resp = self.recv_response(timeout)?;
        let rtt = t0.elapsed();
        Ok((resp, rtt))
    }

    /// Server address this client is connected to.
    pub fn server_addr(&self) -> SocketAddr {
        self.server_addr
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{serialize_response, BLZL_MAGIC};

    fn sample_request() -> TransactionRequest {
        TransactionRequest {
            request_id: "test-req-001".into(),
            debit_account_id: "550e8400-e29b-41d4-a716-446655440001".into(),
            credit_account_id: "550e8400-e29b-41d4-a716-446655440002".into(),
            amount: "50.00".into(),
            currency: "USD".into(),
            ledger_id: 1,
            code: 1,
        }
    }

    // ── Unit: frame encoding ──────────────────────────────────────────────────

    #[test]
    fn encoded_frame_starts_with_blzl_magic() {
        let req = sample_request();
        let frame = encode_blzl_frame(&req).unwrap();
        assert_eq!(&frame[..4], &BLZL_MAGIC);
    }

    #[test]
    fn encoded_frame_payload_round_trips() {
        use crate::protocol::deserialize_request;

        let req = sample_request();
        let frame = encode_blzl_frame(&req).unwrap();
        let payload = &frame[BLZL_MAGIC.len()..];
        let decoded = deserialize_request(payload).unwrap();
        assert_eq!(decoded.request_id, req.request_id);
        assert_eq!(decoded.amount, req.amount);
    }

    // ── Unit: client loopback ─────────────────────────────────────────────────

    /// Round-trip test using two UDP sockets on localhost.
    /// No AF_XDP server required — the test simulates the server side by
    /// manually sending a msgpack response.
    #[test]
    fn client_roundtrip_loopback() {
        use std::net::UdpSocket;

        // Simulated "server" socket.
        let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = server_sock.local_addr().unwrap();

        let client = AfXdpClient::connect(server_addr).unwrap();

        // Spawn a thread that acts as the server: receive one frame, send back a response.
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; MAX_UDP_PAYLOAD];
            let (n, client_addr) = server_sock.recv_from(&mut buf).unwrap();

            // Validate magic
            assert_eq!(&buf[..4], &BLZL_MAGIC);
            // Decode request
            let payload = &buf[4..n];
            let req: TransactionRequest =
                rmp_serde::from_slice(payload).expect("valid msgpack request");

            // Build and send response
            let resp = TransactionResponse {
                request_id: req.request_id.clone(),
                committed: true,
                transfer_id: Some("transfer-001".into()),
                error: None,
                timestamp_ns: 1_000_000_000,
            };
            let resp_bytes = serialize_response(&resp).unwrap();
            server_sock.send_to(&resp_bytes, client_addr).unwrap();
        });

        let req = sample_request();
        let (resp, _rtt) = client
            .roundtrip(&req, Duration::from_secs(2))
            .expect("roundtrip should succeed");

        assert!(resp.committed);
        assert_eq!(resp.request_id, req.request_id);

        handle.join().unwrap();
    }
}

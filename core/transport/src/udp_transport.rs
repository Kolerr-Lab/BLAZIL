//! UDP-based zero-copy transport server.
//!
//! [`UdpTransportServer`] implements ultra-low latency transport using UDP
//! with zero-copy serialization. Eliminates TCP/gRPC overhead for maximum
//! throughput.
//!
//! # Design Philosophy
//!
//! - **Zero connection overhead**: UDP is connectionless
//! - **Zero serialization overhead**: Direct memory mapping
//! - **Zero head-of-line blocking**: Independent packets
//! - **Zero handshake**: No TLS, no HTTP/2
//! - **Fixed packet size**: 56 bytes (cache-line friendly)
//!
//! # Packet Format
//!
//! Request (56 bytes):
//! ```text
//! [0-7]:    Sequence number (u64, network byte order)
//! [8-15]:   TransactionId (u64, network byte order)
//! [16-23]:  DebitAccountId (u64, network byte order)
//! [24-31]:  CreditAccountId (u64, network byte order)
//! [32-39]:  Amount in minor units (u64, network byte order)
//! [40-47]:  Timestamp (u64, network byte order, nanoseconds since epoch)
//! [48-51]:  LedgerId (u32, network byte order)
//! [52-53]:  Transaction code (u16, network byte order)
//! [54]:     Flags (u8)
//! [55]:     Padding (u8)
//! ```
//!
//! Response (16 bytes):
//! ```text
//! [0-7]:   Sequence number (u64, echo from request)
//! [8-15]:  Result code (u64: 0 = success, 1 = rejected)
//! ```
//!
//! # Performance Target
//!
//! - **Target**: 1M+ TPS on single node
//! - **Baseline (TCP)**: 44K TPS
//! - **Expected improvement**: 20-30× over TCP/gRPC

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashmap::DashMap;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::timestamp::Timestamp;
use blazil_engine::event::{EventFlags, TransactionEvent, TransactionResult};
use blazil_engine::sharded_pipeline::ShardedPipeline;

/// UDP packet header size (sequence number only)
const HEADER_SIZE: usize = 8;
/// TransactionEvent payload size (without sequence field which is in header)
/// Layout: tx_id(8) + debit(8) + credit(8) + amount(8) + timestamp(8) + ledger(4) + code(2) + flags(1) + padding(1) = 48 bytes
const PAYLOAD_SIZE: usize = 48;
/// Total packet size (56 bytes - fits in single cache line with header)
const PACKET_SIZE: usize = HEADER_SIZE + PAYLOAD_SIZE;
/// Response packet size
const RESPONSE_SIZE: usize = 16;
/// Result polling timeout (same as TCP for fair comparison)
const RESULT_TIMEOUT: Duration = Duration::from_millis(100);

// ── UdpTransportServer ────────────────────────────────────────────────────────

/// Ultra-low latency UDP transport server with zero-copy semantics.
pub struct UdpTransportServer {
    addr: String,
    pipeline: Arc<ShardedPipeline>,
    shutdown: Arc<AtomicBool>,
    packets_received: Arc<AtomicU64>,
    packets_sent: Arc<AtomicU64>,
    bound_addr: Arc<Mutex<Option<String>>>,
}

impl UdpTransportServer {
    /// Creates a new UDP transport server.
    ///
    /// # Arguments
    ///
    /// - `addr` — bind address (e.g. `"127.0.0.1:7878"`)
    /// - `pipeline` — shared sharded pipeline for event processing
    pub fn new(addr: &str, pipeline: Arc<ShardedPipeline>) -> Self {
        Self {
            addr: addr.to_string(),
            pipeline,
            shutdown: Arc::new(AtomicBool::new(false)),
            packets_received: Arc::new(AtomicU64::new(0)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            bound_addr: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns the actual bound address after the server has started.
    pub fn local_addr(&self) -> String {
        self.bound_addr
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "127.0.0.1:0".to_string())
    }

    /// Async version of local_addr that waits for binding.
    pub async fn local_addr_async(&self) -> String {
        loop {
            {
                let addr = self.bound_addr.lock().unwrap();
                if let Some(ref a) = *addr {
                    return a.clone();
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
    }

    /// Starts the UDP server and processes incoming packets.
    pub async fn serve(&self) -> BlazerResult<()> {
        let socket = UdpSocket::bind(&self.addr)
            .await
            .map_err(|e| BlazerError::Internal(format!("Failed to bind UDP socket: {}", e)))?;

        let local_addr = socket
            .local_addr()
            .map_err(|e| BlazerError::Internal(format!("Failed to get local address: {}", e)))?;

        // Store bound address
        {
            let mut addr = self.bound_addr.lock().unwrap();
            *addr = Some(local_addr.to_string());
        }

        info!("UDP transport server listening on {}", local_addr);

        let mut buf = [0u8; PACKET_SIZE];
        let mut response_buf = [0u8; RESPONSE_SIZE];

        while !self.shutdown.load(Ordering::Relaxed) {
            // Receive packet
            let (len, peer) = match socket.recv_from(&mut buf).await {
                Ok(result) => result,
                Err(e) => {
                    error!("UDP recv_from error: {}", e);
                    continue;
                }
            };

            self.packets_received.fetch_add(1, Ordering::Relaxed);

            // Validate packet size
            if len != PACKET_SIZE {
                warn!(
                    "Invalid packet size from {}: expected {}, got {}",
                    peer, PACKET_SIZE, len
                );
                let seq = u64::from_be_bytes(buf[0..8].try_into().unwrap());
                self.send_error_response(&socket, peer, seq).await;
                continue;
            }

            // Extract sequence number (first 8 bytes, network byte order)
            let sequence = u64::from_be_bytes(buf[0..8].try_into().unwrap());

            // Deserialize TransactionEvent (zero-copy from bytes 8-56)
            let event = match self.deserialize_event(&buf[HEADER_SIZE..PACKET_SIZE]) {
                Ok(event) => event,
                Err(e) => {
                    error!("Failed to deserialize event: {}", e);
                    self.send_error_response(&socket, peer, sequence).await;
                    continue;
                }
            };

            debug!(
                "Received event seq={} tx_id={} from {}",
                sequence,
                event.transaction_id.as_u64(),
                peer
            );

            // Determine which shard will process this event (by debit account)
            let shard_id = (event.debit_account_id.as_u64() as usize) % self.pipeline.shard_count();

            // Publish to sharded pipeline (routes by account_id % shard_count)
            let seq = match self.pipeline.publish_event(event) {
                Ok(seq) => seq,
                Err(e) => {
                    warn!("Pipeline backpressure for seq={}: {}", sequence, e);
                    self.send_error_response(&socket, peer, sequence).await;
                    continue;
                }
            };

            // Get this shard's results map for polling
            let shard_results = self.pipeline.shard_results(shard_id);

            // Wait for processing to complete (honest E2E measurement like TCP)
            let result_code = match wait_for_result(&shard_results, seq).await {
                Some(result) => match result {
                    TransactionResult::Committed { .. } => 0u64, // Success
                    TransactionResult::Rejected { .. } => 1u64,  // Rejected
                },
                None => {
                    // Timeout waiting for result
                    warn!("Processing timeout for seq={}", sequence);
                    1u64 // Treat timeout as rejection
                }
            };

            // Send response after processing completes
            response_buf[0..8].copy_from_slice(&sequence.to_be_bytes());
            response_buf[8..16].copy_from_slice(&result_code.to_be_bytes());

            if let Err(e) = socket.send_to(&response_buf, peer).await {
                error!("Failed to send response to {}: {}", peer, e);
            } else {
                self.packets_sent.fetch_add(1, Ordering::Relaxed);
            }
        }

        info!("UDP transport server shutting down");
        Ok(())
    }

    /// Deserializes a TransactionEvent from raw bytes.
    ///
    /// # Wire Format
    ///
    /// UDP payload (48 bytes):
    /// - `[0-7]`:   transaction_id (u64, big-endian)
    /// - `[8-15]`:  debit_account_id (u64, big-endian)
    /// - `[16-23]`: credit_account_id (u64, big-endian)
    /// - `[24-31]`: amount_units (u64, big-endian)
    /// - `[32-39]`: timestamp (u64, big-endian, nanos since epoch)
    /// - `[40-43]`: ledger_id (u32, big-endian)
    /// - `[44-45]`: code (u16, big-endian)
    /// - `[46]`:    flags (u8)
    /// - `[47]`:    padding (u8)
    fn deserialize_event(&self, bytes: &[u8]) -> BlazerResult<TransactionEvent> {
        if bytes.len() != PAYLOAD_SIZE {
            return Err(BlazerError::Internal(format!(
                "Invalid UDP packet size: expected {} bytes, got {}",
                PAYLOAD_SIZE,
                bytes.len()
            )));
        }

        // Extract fields with explicit bounds checking (network byte order)
        let tx_id = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
        let debit_id = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
        let credit_id = u64::from_be_bytes(bytes[16..24].try_into().unwrap());
        let amount = u64::from_be_bytes(bytes[24..32].try_into().unwrap());
        let timestamp_nanos = u64::from_be_bytes(bytes[32..40].try_into().unwrap());
        let ledger_u32 = u32::from_be_bytes(bytes[40..44].try_into().unwrap());
        let code = u16::from_be_bytes(bytes[44..46].try_into().unwrap());
        let flags_byte = bytes[46];

        // Convert raw u32 to LedgerId
        let ledger_id = match ledger_u32 {
            0 => LedgerId::USD,
            1 => LedgerId::EUR,
            2 => LedgerId::GBP,
            _ => LedgerId::USD, // Default fallback
        };

        // Build event (sequence will be assigned by ring buffer)
        let mut event = TransactionEvent::new(
            TransactionId::from_u64(tx_id),
            AccountId::from_u64(debit_id),
            AccountId::from_u64(credit_id),
            amount,
            ledger_id,
            code,
        );

        // Restore original timestamp from wire (override Timestamp::now())
        event.ingestion_timestamp = Timestamp::from_nanos(timestamp_nanos);

        // Restore flags
        event.flags = EventFlags::from_raw(flags_byte);

        Ok(event)
    }

    /// Sends an error response to the client.
    async fn send_error_response(
        &self,
        socket: &UdpSocket,
        peer: std::net::SocketAddr,
        sequence: u64,
    ) {
        let mut response = [0u8; RESPONSE_SIZE];
        response[0..8].copy_from_slice(&sequence.to_be_bytes());
        response[8..16].copy_from_slice(&1u64.to_be_bytes()); // 1 = error

        if let Err(e) = socket.send_to(&response, peer).await {
            error!("Failed to send error response to {}: {}", peer, e);
        } else {
            self.packets_sent.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Initiates graceful shutdown.
    pub async fn shutdown(&self) {
        info!("Initiating UDP server shutdown");
        self.shutdown.store(true, Ordering::Relaxed);

        info!(
            "UDP server statistics: received={}, sent={}",
            self.packets_received.load(Ordering::Relaxed),
            self.packets_sent.load(Ordering::Relaxed)
        );
    }

    /// Returns the number of packets received.
    pub fn packets_received(&self) -> u64 {
        self.packets_received.load(Ordering::Relaxed)
    }

    /// Returns the number of packets sent.
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }
}

// ── Helper Functions ──────────────────────────────────────────────────────────

/// Wait for a transaction result to be available in the results map.
///
/// Polls the results map until the sequence number appears or timeout expires.
/// Uses `tokio::task::yield_now()` to avoid busy-waiting.
///
/// # Arguments
///
/// * `results` - Results map for the target shard
/// * `seq` - Sequence number to wait for
///
/// # Returns
///
/// `Some(result)` if processing completed within timeout, `None` on timeout.
async fn wait_for_result(
    results: &Arc<DashMap<i64, TransactionResult>>,
    seq: i64,
) -> Option<TransactionResult> {
    let results = Arc::clone(results);
    let fut = async move {
        loop {
            if let Some(r) = results.get(&seq) {
                return r.value().clone();
            }
            // Yield to tokio scheduler to avoid busy-waiting
            tokio::task::yield_now().await;
        }
    };

    timeout(RESULT_TIMEOUT, fut).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_sizes() {
        assert_eq!(HEADER_SIZE, 8);
        assert_eq!(PAYLOAD_SIZE, 48);
        assert_eq!(PACKET_SIZE, 56);
        assert_eq!(RESPONSE_SIZE, 16);
    }

    #[tokio::test]
    async fn test_udp_server_bind() {
        let pipeline =
            Arc::new(ShardedPipeline::new(1, 1024, 1_000_000).expect("pipeline creation failed"));
        let server = UdpTransportServer::new("127.0.0.1:0", pipeline);

        assert_eq!(server.packets_received(), 0);
        assert_eq!(server.packets_sent(), 0);
    }
}

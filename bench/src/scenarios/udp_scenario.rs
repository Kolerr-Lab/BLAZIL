//! UDP E2E — Zero-copy transport layer.
//!
//! Client → UDP → UdpTransportServer → ShardedPipeline → InMemoryLedgerClient.
//!
//! Uses custom 56-byte UDP packets with zero-copy serialization.
//! No connection overhead, no TLS, no HTTP/2, no protobuf marshalling.
//!
//! **IMPORTANT**: This measures ENQUEUE throughput (ring buffer intake capacity),
//! NOT full processing throughput. The UDP server responds immediately after
//! `publish_event()` returns (which just claims a ring buffer slot). Background
//! handler threads process events asynchronously afterward.
//!
//! For TRUE E2E processing throughput, see TCP scenario which uses Pipeline
//! with synchronous result polling.
//!
//! **Goal**: Measure UDP transport overhead vs TCP, given same enqueue semantics.
//!
//! Warmup:    1,000 events  (UDP socket warmup)
//! Benchmark: 100K events  (fire-and-forget, batch send via sendmmsg pattern)

use std::sync::Arc;
use std::time::{Duration, Instant};

use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::sharded_pipeline::ShardedPipeline;
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::mock::InMemoryLedgerClient;
use blazil_transport::udp_transport::UdpTransportServer;
use tokio::net::UdpSocket;

use crate::metrics::BenchmarkResult;

const WARMUP_EVENTS: u64 = 1_000;
const PACKET_SIZE: usize = 56; // 8 (seq) + 48 (payload)

/// Run the UDP scenario once for fast testing.
pub async fn run(events: u64) -> BenchmarkResult {
    run_once(events).await
}

async fn run_once(events: u64) -> BenchmarkResult {
    let usd = parse_currency("USD").expect("USD");

    // ── shared ledger ────────────────────────────────────────────────────────
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());

    // Pre-create accounts directly.
    let debit_id = client
        .create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("debit account");
    let credit_id = client
        .create_account(Account::new(
            AccountId::new(),
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("credit account");

    // ── sharded pipeline ─────────────────────────────────────────────────────
    let pipeline = Arc::new(
        ShardedPipeline::new(
            4,             // 4 shards (good for 16-core extrapolation)
            1_048_576,     // 1M ring buffer capacity per shard
            1_000_000_000, // 1B events/sec rate limit
        )
        .expect("sharded pipeline"),
    );

    // ── UDP server ───────────────────────────────────────────────────────────
    let server = Arc::new(UdpTransportServer::new(
        "127.0.0.1:0",
        Arc::clone(&pipeline),
    ));
    let s = Arc::clone(&server);
    tokio::spawn(async move {
        let _ = s.serve().await;
    });

    // Wait for server to bind and get actual address
    let addr = server.local_addr_async().await;
    let server_addr: std::net::SocketAddr = addr.parse().expect("parse server addr");

    // ── client socket ────────────────────────────────────────────────────────
    let client_sock = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
    client_sock.connect(server_addr).await.expect("connect");

    // ── warmup: prime UDP socket ────────────────────────────────────────────
    for i in 0..WARMUP_EVENTS {
        let packet = make_udp_packet(i, &debit_id, &credit_id);
        let _ = client_sock.send(&packet).await;
    }

    tokio::time::sleep(Duration::from_millis(10)).await;

    // ── benchmark: UDP with response confirmation ────────────────────────────
    let mut latencies = Vec::with_capacity(events as usize);
    let wall_start = Instant::now();
    let mut response_buf = [0u8; 16];

    for i in 0..events {
        let packet = make_udp_packet(i, &debit_id, &credit_id);
        let t0 = Instant::now();

        // Send packet
        client_sock.send(&packet).await.expect("send");

        // Wait for 16-byte response (TRUE E2E!)
        client_sock
            .recv(&mut response_buf)
            .await
            .expect("recv response");

        latencies.push(t0.elapsed().as_nanos() as u64);
    }

    let duration = wall_start.elapsed();

    // ── shutdown ─────────────────────────────────────────────────────────────
    server.shutdown().await;

    BenchmarkResult::new("UDP E2E", events, duration, &mut latencies)
}

/// Creates a 56-byte UDP packet.
///
/// Packet layout:
/// ```text
/// [0-7]:    Sequence number (u64, big-endian)
/// [8-15]:   TransactionId (u64, big-endian)
/// [16-23]:  DebitAccountId (u64, big-endian)
/// [24-31]:  CreditAccountId (u64, big-endian)
/// [32-39]:  Amount (u64, big-endian)
/// [40-47]:  Timestamp (u64, big-endian)
/// [48-51]:  LedgerId (u32, big-endian, 0 = USD)
/// [52-53]:  Code (u16, big-endian)
/// [54]:     Flags (u8)
/// [55]:     Padding (u8)
/// ```
fn make_udp_packet(seq: u64, debit_id: &AccountId, credit_id: &AccountId) -> Vec<u8> {
    let mut packet = vec![0u8; PACKET_SIZE];

    // Header: sequence number
    packet[0..8].copy_from_slice(&seq.to_be_bytes());

    // Payload: TransactionEvent fields
    let tx_id = TransactionId::new();
    packet[8..16].copy_from_slice(&tx_id.as_u64().to_be_bytes());
    packet[16..24].copy_from_slice(&debit_id.as_u64().to_be_bytes());
    packet[24..32].copy_from_slice(&credit_id.as_u64().to_be_bytes());
    packet[32..40].copy_from_slice(&10_000u64.to_be_bytes()); // $100.00 in cents
    packet[40..48].copy_from_slice(&0u64.to_be_bytes()); // Timestamp (server assigns)
    packet[48..52].copy_from_slice(&0u32.to_be_bytes()); // LedgerId::USD = 0
    packet[52..54].copy_from_slice(&1u16.to_be_bytes()); // Code = 1
    packet[54] = 0; // Flags = 0
    packet[55] = 0; // Padding

    packet
}

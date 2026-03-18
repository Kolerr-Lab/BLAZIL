//! UDP E2E — Window-based async pipelining.
//!
//! Client → UDP → UdpTransportServer → ShardedPipeline → InMemoryLedgerClient.
//!
//! Uses custom 56-byte UDP packets with zero-copy serialization.
//! No connection overhead, no TLS, no HTTP/2, no protobuf marshalling.
//!
//! **KEY OPTIMIZATION**: Window-based async pipelining (same as gRPC 200 → 62K breakthrough).
//! Client sends WINDOW_SIZE requests without waiting, then collects responses async.
//! This keeps the pipeline full and eliminates serial bottleneck.
//!
//! Current: Synchronous send/recv = 1 request at a time = ~62K TPS ceiling
//! With windowing: N requests in-flight = N× parallelism = 500K+ TPS target
//!
//! Warmup:    1,000 events  (UDP socket warmup)
//! Benchmark: 100K events  (window-based pipelining)

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
const WINDOW_SIZE: usize = 5_000; // Number of in-flight requests (balanced for stability + performance)

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

    // ── benchmark: window-based async pipelining ─────────────────────────────
    // Pre-generate all packets (reuse for efficiency)
    let packets: Vec<Vec<u8>> = (0..events)
        .map(|i| make_udp_packet(i, &debit_id, &credit_id))
        .collect();

    let mut latencies = Vec::with_capacity(events as usize);
    let mut send_times = Vec::with_capacity(events as usize);
    let wall_start = Instant::now();

    let mut response_buf = [0u8; 16];
    let mut sent = 0usize;
    let mut received = 0usize;
    let total_events = events as usize;

    // ── Phase 1: Fill the window ────────────────────────────────────────────
    let initial_window = WINDOW_SIZE.min(total_events);
    for packet in packets.iter().take(initial_window) {
        send_times.push(Instant::now());
        client_sock
            .send(packet)
            .await
            .expect("send initial window");
        sent += 1;
    }

    // ── Phase 2: Pipeline loop (send next when response arrives) ────────────
    while received < total_events {
        // Collect one response
        client_sock
            .recv(&mut response_buf)
            .await
            .expect("recv response");

        // Record latency for this response
        if let Some(t0) = send_times.get(received) {
            latencies.push(t0.elapsed().as_nanos() as u64);
        }
        received += 1;

        // Send next packet if more to send
        if sent < total_events {
            send_times.push(Instant::now());
            client_sock
                .send(&packets[sent])
                .await
                .expect("send next");
            sent += 1;
        }
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

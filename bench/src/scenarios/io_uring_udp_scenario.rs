//! io_uring UDP E2E benchmark — window-based async pipelining over io_uring.
//!
//! Client → tokio UDP socket → IoUringUdpTransport → ShardedPipeline → InMemoryLedgerClient.
//!
//! Mirrors `udp_scenario.rs` exactly but replaces `UdpTransportServer` with
//! `IoUringUdpTransport` to measure the latency / throughput delta from
//! moving recv/send into the kernel's io_uring submission queue.
//!
//! # What changes vs the standard UDP scenario
//!
//! - Server: `IoUringUdpTransport` (io_uring SQ/CQ, pre-registered buffers)
//! - Client: unchanged — standard `tokio::net::UdpSocket` (epoll)
//! - Packet format: identical 56-byte wire format
//!
//! # Requirements
//!
//! Linux 5.1+ with the `io-uring` feature enabled:
//! ```text
//! cargo run -p blazil-bench --release --features io-uring
//! ```
//!
//! Warmup:    1,000 events  (socket + io_uring ring warm-up)
//! Benchmark: 100K events  (window-based pipelining, WINDOW_SIZE = 256)

use std::sync::Arc;
use std::time::{Duration, Instant};

use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::sharded_pipeline::ShardedPipeline;
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::mock::InMemoryLedgerClient;
use blazil_transport::IoUringUdpTransport;
use tokio::net::UdpSocket;

use crate::metrics::BenchmarkResult;

const WARMUP_EVENTS: u64 = 1_000;
const PACKET_SIZE: usize = 56; // 8 (seq) + 48 (payload)
/// Window size matches IoUringUdpTransport's buffer pool (RECV_BUFFER_COUNT = 256).
/// Keeping WINDOW_SIZE ≤ RECV_BUFFER_COUNT avoids in-flight slot exhaustion.
const WINDOW_SIZE: usize = 256;

/// Run the io_uring UDP scenario once.
pub async fn run(events: u64) -> BenchmarkResult {
    run_once(events).await
}

async fn run_once(events: u64) -> BenchmarkResult {
    let usd = parse_currency("USD").expect("USD");

    // ── shared ledger ────────────────────────────────────────────────────────
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());

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
            4,             // 4 shards
            1_048_576,     // 1M ring buffer capacity per shard
            1_000_000_000, // 1B events/sec rate limit
        )
        .expect("sharded pipeline"),
    );

    // ── io_uring UDP server ───────────────────────────────────────────────────
    let server = Arc::new(IoUringUdpTransport::new(
        "127.0.0.1:0",
        Arc::clone(&pipeline),
    ));
    let s = Arc::clone(&server);
    tokio::spawn(async move {
        let _ = s.serve().await;
    });

    // Wait for server to bind.
    let addr = server.local_addr_async().await;
    let server_addr: std::net::SocketAddr = addr.parse().expect("parse server addr");

    // ── client socket ─────────────────────────────────────────────────────────
    let client_sock = UdpSocket::bind("127.0.0.1:0").await.expect("client bind");
    client_sock.connect(server_addr).await.expect("connect");

    // ── warmup ────────────────────────────────────────────────────────────────
    for i in 0..WARMUP_EVENTS {
        let packet = make_udp_packet(i, &debit_id, &credit_id);
        let _ = client_sock.send(&packet).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

    // ── benchmark: window-based async pipelining ──────────────────────────────
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

    // Phase 1: fill the window.
    let initial_window = WINDOW_SIZE.min(total_events);
    for packet in packets.iter().take(initial_window) {
        send_times.push(Instant::now());
        client_sock.send(packet).await.expect("send initial window");
        sent += 1;
    }

    // Phase 2: pipeline loop — receive one, send one.
    while received < total_events {
        client_sock
            .recv(&mut response_buf)
            .await
            .expect("recv response");

        if let Some(t0) = send_times.get(received) {
            latencies.push(t0.elapsed().as_nanos() as u64);
        }
        received += 1;

        if sent < total_events {
            send_times.push(Instant::now());
            client_sock.send(&packets[sent]).await.expect("send next");
            sent += 1;
        }
    }

    let duration = wall_start.elapsed();

    // ── shutdown ──────────────────────────────────────────────────────────────
    server.shutdown().await;

    BenchmarkResult::new("io_uring UDP E2E", events, duration, &mut latencies)
}

/// Creates a 56-byte UDP packet (identical format to `udp_scenario.rs`).
///
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

    packet[0..8].copy_from_slice(&seq.to_be_bytes());

    let tx_id = TransactionId::new();
    packet[8..16].copy_from_slice(&tx_id.as_u64().to_be_bytes());
    packet[16..24].copy_from_slice(&debit_id.as_u64().to_be_bytes());
    packet[24..32].copy_from_slice(&credit_id.as_u64().to_be_bytes());
    packet[32..40].copy_from_slice(&10_000u64.to_be_bytes()); // $100.00
    packet[40..48].copy_from_slice(&0u64.to_be_bytes());      // timestamp
    packet[48..52].copy_from_slice(&0u32.to_be_bytes());      // LedgerId::USD
    packet[52..54].copy_from_slice(&1u16.to_be_bytes());      // code = 1
    packet[54] = 0; // flags
    packet[55] = 0; // padding

    packet
}

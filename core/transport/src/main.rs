//! Blazil transport server binary.
//!
//! Starts the transport server with a configurable ledger backend.
//! Env vars:
//!   BLAZIL_BIND_ADDR      — TCP listen address (default: 0.0.0.0:7878)
//!   BLAZIL_METRICS_PORT   — Prometheus metrics port  (default: 9090)
//!   BLAZIL_CAPACITY       — Ring buffer capacity, must be power-of-two (default: 65536)
//!   BLAZIL_TB_ADDRESS     — TigerBeetle address (e.g. tigerbeetle:3000)
//!                           If set: uses real TigerBeetle (requires tigerbeetle-client feature)
//!                           If unset: uses in-memory ledger (dev/test only)
//!   BLAZIL_TRANSPORT      — Transport backend:
//!                             "tcp"          (default) — standard tokio TCP
//!                             "aeron"        — Aeron UDP (requires feature = "aeron")
//!                             "io-uring"     — io_uring TCP (Linux + feature = "io-uring")
//!                             "aeron+io-uring" — Aeron UDP receive + io_uring TCP send
//!   AERON_DIR             — Aeron C Media Driver IPC dir (default: /dev/shm/aeron)
//!   BLAZIL_AERON_CHANNEL  — Aeron channel URI (default: aeron:udp?endpoint=0.0.0.0:20121)

use std::sync::Arc;

use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId};
use blazil_engine::handlers::ledger::LedgerHandler;
use blazil_engine::handlers::publish::PublishHandler;
use blazil_engine::handlers::risk::RiskHandler;
use blazil_engine::handlers::validation::ValidationHandler;
use blazil_engine::metrics::EngineMetrics;
use blazil_engine::pipeline::PipelineBuilder;
use blazil_ledger::account::{Account, AccountFlags};
use blazil_ledger::client::LedgerClient;
use blazil_ledger::mock::InMemoryLedgerClient;
#[cfg(feature = "aeron")]
use blazil_transport::aeron_transport::{AeronTransportServer, DEFAULT_AERON_CHANNEL};
#[cfg(all(target_os = "linux", feature = "io-uring"))]
use blazil_transport::io_uring_transport::IoUringTransportServer;
use blazil_transport::metrics_server::MetricsServer;
use blazil_transport::server::TransportServer;
use blazil_transport::tcp::TcpTransportServer;
use tracing::{info, warn};

#[cfg(feature = "tigerbeetle-client")]
use blazil_ledger::tigerbeetle::TigerBeetleClient;

const DEFAULT_CAPACITY: usize = 65_536;
const DEFAULT_MAX_CONNECTIONS: u64 = 10_000;

// ── TigerBeetle connection with retry ─────────────────────────────────────────

/// Probes the TigerBeetle TCP port, then initialises `TigerBeetleClient`.
///
/// Returns `Some(client)` on success.  Returns `None` after `max_retries`
/// failed attempts so the caller can fall back to `InMemoryLedgerClient`.
///
/// Environment variables (read by `main`, passed in here):
///   `BLAZIL_TB_CONNECT_RETRY`    — max attempts        (default 20)
///   `BLAZIL_TB_CONNECT_DELAY_MS` — wait between tries  (default 500 ms)
#[cfg(feature = "tigerbeetle-client")]
async fn try_connect_tb(
    addr: &str,
    max_retries: u32,
    delay_ms: u64,
) -> Option<Arc<TigerBeetleClient>> {
    for attempt in 1..=max_retries {
        info!(
            "Connecting to TigerBeetle... attempt {}/{}",
            attempt, max_retries
        );
        // Skip the TCP probe: tb::Client::new() is lazy and returns Ok before
        // the VSR handshake completes. Connect then send a real probe operation
        // (lookup account 0) to confirm the cluster is actually ready.
        match TigerBeetleClient::connect(addr, 0).await {
            Ok(client) => match client.probe().await {
                Ok(()) => {
                    info!("✅ TigerBeetle connected ({})", addr);
                    return Some(Arc::new(client));
                }
                Err(e) => {
                    warn!(
                        "TB handshake probe failed (attempt {}/{}): {}",
                        attempt, max_retries, e
                    );
                }
            },
            Err(e) => {
                warn!(
                    "TB not yet reachable (attempt {}/{}): {}",
                    attempt, max_retries, e
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }
    warn!(
        "⚠️  TB unavailable after {} attempts, falling back to in-memory ledger",
        max_retries
    );
    None
}

/// Builds, wires, and runs the full pipeline with the given ledger client.
///
/// This is generic over `C: LedgerClient` so that both `InMemoryLedgerClient`
/// and `TigerBeetleClient` can be used without boxing (which would break the
/// `LedgerHandler<C: Sized>` bound).
///
/// The `transport` argument selects the network backend:
///   - `"tcp"`           (default) — [`TcpTransportServer`]
///   - `"aeron"`         — [`AeronTransportServer`] (requires `--features aeron`)
///   - `"io-uring"`      — [`IoUringTransportServer`] (Linux + `--features io-uring`)
///   - `"aeron+io-uring"` — Aeron UDP with io_uring TCP fallback (Linux + both features)
async fn run_pipeline<C: LedgerClient + 'static>(
    client: Arc<C>,
    bind_addr: String,
    metrics_addr: String,
    capacity: usize,
    transport: String,
) {
    let ledger_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("blazil-ledger")
            .enable_all()
            .build()
            .expect("ledger runtime"),
    );

    // $1 billion maximum per transaction in cents.
    let max_amount_units: u64 = 100_000_000_000_000_u64;

    // ── Pipeline ──────────────────────────────────────────────────────────────
    let builder = PipelineBuilder::new().with_capacity(capacity);
    let results = builder.results();
    let (pipeline, runners) = builder
        .add_handler(ValidationHandler::new(Arc::clone(&results)))
        .add_handler(RiskHandler::new(max_amount_units, Arc::clone(&results)))
        .add_handler(LedgerHandler::new(client, ledger_rt, Arc::clone(&results)))
        .add_handler(PublishHandler::new(Arc::clone(&results)))
        .build()
        .expect("pipeline build");

    #[allow(unused_variables)]
    let ring_buffer = Arc::clone(pipeline.ring_buffer());
    let pipeline = Arc::new(pipeline);

    // ── CPU Affinity (FIX 1) ──────────────────────────────────────────────────
    // Pin pipeline runner thread to core 0 (hot path, zero context switching).
    // OS must NEVER move this thread to another core.
    let core_ids = core_affinity::get_core_ids().expect("failed to get core IDs");
    if core_ids.len() >= 2 {
        info!("🔒 Pinning pipeline to core 0, network to core 1");
        // Pin current thread (main/pipeline) to core 0
        core_affinity::set_for_current(core_ids[0]);
    } else {
        warn!("⚠️  <2 CPU cores detected, skipping affinity pinning");
    }

    let _run_handles: Vec<_> = runners.into_iter().map(|r| r.run()).collect();

    // ── Metrics server ────────────────────────────────────────────────────────
    let metrics = Arc::new(EngineMetrics::new());
    let metrics_svc = MetricsServer::new(Arc::clone(&metrics), metrics_addr);
    tokio::spawn(async move {
        metrics_svc.serve().await;
    });

    // ── Transport dispatch ──────────────────────────────────────────────────────

    // io-uring and aeron+io-uring: Linux-only.
    // On macOS/CI (or when the feature is off) we fall through to TCP.
    if transport == "io-uring" || transport == "aeron+io-uring" {
        #[cfg(all(target_os = "linux", feature = "io-uring"))]
        {
            let label = if transport == "aeron+io-uring" {
                "🚀 Aeron UDP + io_uring active — MAXIMUM PERFORMANCE"
            } else {
                "🚀 io_uring active"
            };
            info!("{label}");
            let server = Arc::new(IoUringTransportServer::new(
                &bind_addr,
                Arc::clone(&pipeline),
                ring_buffer,
            ));
            info!("blazil-engine ready");
            server.serve().await.expect("io_uring server error");
            return;
        }
        #[cfg(not(all(target_os = "linux", feature = "io-uring")))]
        {
            warn!(
                transport = %transport,
                "⚠️  io_uring not available on this platform — falling back to TCP"
            );
        }
    }

    #[cfg(feature = "aeron")]
    if transport == "aeron" {
        let aeron_dir = std::env::var("AERON_DIR").unwrap_or_else(|_| "/dev/shm/aeron".to_string());
        let channel = std::env::var("BLAZIL_AERON_CHANNEL")
            .unwrap_or_else(|_| DEFAULT_AERON_CHANNEL.to_string());
        let server = Arc::new(AeronTransportServer::new(
            &channel,
            &aeron_dir,
            Arc::clone(&pipeline),
            ring_buffer,
        ));
        info!("🚀 Aeron UDP active");
        info!("blazil-engine ready");
        server.serve().await.expect("aeron server error");
        return;
    }

    // Default: TCP transport
    if transport != "tcp" {
        warn!(
            transport = %transport,
            "unknown BLAZIL_TRANSPORT value — falling back to TCP"
        );
    } else {
        info!("⚡ TCP transport active");
    }
    let server = Arc::new(TcpTransportServer::new(
        &bind_addr,
        Arc::clone(&pipeline),
        Arc::clone(&results),
        DEFAULT_MAX_CONNECTIONS,
    ));

    info!("blazil-engine ready");
    server.serve().await.expect("server error");
}

/// Pre-populates the in-memory client with a demo account pair and returns it.
async fn make_in_memory_client() -> Arc<InMemoryLedgerClient> {
    let usd = parse_currency("USD").expect("USD currency");
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());
    let debit_id = AccountId::new();
    let credit_id = AccountId::new();
    client
        .create_account(Account::new(
            debit_id,
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("debit account");
    client
        .create_account(Account::new(
            credit_id,
            LedgerId::USD,
            usd,
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("credit account");
    client
}

#[tokio::main]
async fn main() {
    // ── Tracing ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── Config from env ───────────────────────────────────────────────────────
    let bind_addr =
        std::env::var("BLAZIL_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    let metrics_port = std::env::var("BLAZIL_METRICS_PORT").unwrap_or_else(|_| "9090".to_string());
    let metrics_addr = format!("0.0.0.0:{metrics_port}");
    let capacity = std::env::var("BLAZIL_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CAPACITY);
    let transport = std::env::var("BLAZIL_TRANSPORT")
        .unwrap_or_else(|_| "tcp".to_string())
        .to_lowercase();

    info!(
        "blazil-engine starting on {bind_addr}, metrics on {metrics_addr}, transport={transport}"
    );

    // ── Ledger dispatch ───────────────────────────────────────────────────────
    // The tigerbeetle-client feature gates whether we can even attempt a real
    // TB connection (it enables the Zig-based C library). Within that gate we
    // check BLAZIL_TB_ADDRESS at runtime to decide which client to build.
    #[cfg(feature = "tigerbeetle-client")]
    {
        match std::env::var("BLAZIL_TB_ADDRESS") {
            Ok(addr) => {
                let max_retries: u32 = std::env::var("BLAZIL_TB_CONNECT_RETRY")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(20);
                let delay_ms: u64 = std::env::var("BLAZIL_TB_CONNECT_DELAY_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(500);

                info!(
                    "🔥 Engine: TigerBeetle mode ({}) — up to {} attempts",
                    addr, max_retries
                );
                match try_connect_tb(&addr, max_retries, delay_ms).await {
                    Some(client) => {
                        run_pipeline(client, bind_addr, metrics_addr, capacity, transport).await;
                    }
                    None => {
                        warn!(
                            "⚠️  Engine: in-memory mode (TB unavailable — check BLAZIL_TB_ADDRESS)"
                        );
                        let client = make_in_memory_client().await;
                        run_pipeline(client, bind_addr, metrics_addr, capacity, transport).await;
                    }
                }
            }
            Err(_) => {
                info!("⚠️  Engine: in-memory mode (demo/dev only)");
                let client = make_in_memory_client().await;
                run_pipeline(client, bind_addr, metrics_addr, capacity, transport).await;
            }
        }
    }

    #[cfg(not(feature = "tigerbeetle-client"))]
    {
        info!("⚠️  Engine: in-memory mode (demo/dev only)");
        let client = make_in_memory_client().await;
        run_pipeline(client, bind_addr, metrics_addr, capacity, transport).await;
    }
}

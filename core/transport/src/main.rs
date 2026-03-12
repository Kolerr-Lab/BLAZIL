//! Blazil transport server binary.
//!
//! Starts the TCP transport server with an in-memory pipeline.
//! Env vars:
//!   BLAZIL_BIND_ADDR    — TCP listen address (default: 0.0.0.0:7878)
//!   BLAZIL_METRICS_PORT — Prometheus metrics port  (default: 9090)
//!   BLAZIL_CAPACITY     — Ring buffer capacity, must be power-of-two (default: 65536)
//!
//! Note: TigerBeetle integration is flagged for Prompt #14.
//! The demo uses an in-memory ledger that accepts all valid transfers.

use std::sync::Arc;

use blazil_common::amount::Amount;
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
use blazil_transport::metrics_server::MetricsServer;
use blazil_transport::server::TransportServer;
use blazil_transport::tcp::TcpTransportServer;
use rust_decimal::Decimal;
use tracing::info;

const DEFAULT_CAPACITY: usize = 65_536;
const DEFAULT_MAX_CONNECTIONS: u64 = 10_000;

#[tokio::main]
async fn main() {
    // ── Tracing ───────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // ── Config from env ───────────────────────────────────────────────────────
    let bind_addr = std::env::var("BLAZIL_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    let metrics_port = std::env::var("BLAZIL_METRICS_PORT")
        .unwrap_or_else(|_| "9090".to_string());
    let metrics_addr = format!("0.0.0.0:{metrics_port}");
    let capacity = std::env::var("BLAZIL_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CAPACITY);

    info!("blazil-engine starting on {bind_addr}, metrics on {metrics_addr}");

    // ── In-memory ledger ──────────────────────────────────────────────────────
    let usd = parse_currency("USD").expect("USD currency");
    let client = Arc::new(InMemoryLedgerClient::new_unbounded());

    // Pre-create a high-volume system account pair for demo traffic.
    let debit_id = AccountId::new();
    let credit_id = AccountId::new();
    client
        .create_account(Account::new(
            debit_id,
            LedgerId::USD,
            usd.clone(),
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("debit account");
    client
        .create_account(Account::new(
            credit_id,
            LedgerId::USD,
            usd.clone(),
            1,
            AccountFlags::default(),
        ))
        .await
        .expect("credit account");

    let ledger_rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("blazil-ledger")
            .enable_all()
            .build()
            .expect("ledger runtime"),
    );

    let max_amount = Amount::new(
        Decimal::new(1_000_000_000_000_00, 2),
        parse_currency("USD").expect("USD"),
    )
    .expect("max amount");

    // ── Pipeline ──────────────────────────────────────────────────────────────
    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(capacity)
        .add_handler(ValidationHandler)
        .add_handler(RiskHandler::new(max_amount))
        .add_handler(LedgerHandler::new(client, ledger_rt))
        .add_handler(PublishHandler::new())
        .build()
        .expect("pipeline build");

    let ring_buffer = Arc::clone(pipeline.ring_buffer());
    let pipeline = Arc::new(pipeline);
    let _run_handle = runner.run();

    // ── Metrics server ────────────────────────────────────────────────────────
    let metrics = Arc::new(EngineMetrics::new());
    let metrics_svc = MetricsServer::new(Arc::clone(&metrics), metrics_addr);
    tokio::spawn(async move {
        metrics_svc.serve().await;
    });

    // ── TCP transport ─────────────────────────────────────────────────────────
    let server = Arc::new(TcpTransportServer::new(
        &bind_addr,
        Arc::clone(&pipeline),
        ring_buffer,
        DEFAULT_MAX_CONNECTIONS,
    ));

    info!("blazil-engine ready");
    server.serve().await.expect("server error");
}

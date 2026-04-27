//! Embedded WebSocket metrics server for ML benchmark dashboard.
//!
//! Broadcasts real-time per-second metrics to dashboard clients for both
//! dataloader (samples/sec) and inference (RPS) benchmark modes.
//!
//! # Usage
//!
//! ```bash
//! ./ml-bench --mode dataloader \
//!   --dataset imagenet \
//!   --path /data/imagenet \
//!   --metrics-port 9092
//! ```
//!
//! Then connect dashboard to ws://<host>:9092/ws
//!
//! Requires `--features metrics-ws`.

#[cfg(feature = "metrics-ws")]
pub use inner::start;

#[cfg(feature = "metrics-ws")]
mod inner {
    use axum::{
        extract::{
            ws::{Message, WebSocket, WebSocketUpgrade},
            State,
        },
        response::IntoResponse,
        routing::get,
        Router,
    };
    use std::sync::Arc;
    use tokio::sync::{broadcast, RwLock};
    use tower_http::cors::{Any, CorsLayer};

    /// Shared cache for the last "config" message sent by the benchmark.
    /// New WS clients receive this immediately on connect so they never miss it.
    pub type ConfigCache = Arc<RwLock<Option<String>>>;

    /// Start the WS metrics server on `port`.
    ///
    /// Returns:
    /// - `broadcast::Sender<String>`: benchmark threads publish JSON metrics here.
    /// - `broadcast::Receiver<String>`: dashboard → bench control commands (future).
    /// - `ConfigCache`: benchmark writes config JSON here; new clients get it on connect.
    pub fn start(
        port: u16,
    ) -> (
        broadcast::Sender<String>,
        broadcast::Receiver<String>,
        ConfigCache,
    ) {
        // outgoing: bench → dashboard
        let (out_tx, _) = broadcast::channel::<String>(8192);
        // incoming: dashboard → bench (reserved for future control commands)
        let (in_tx, in_rx) = broadcast::channel::<String>(256);
        // config replay cache
        let config_cache: ConfigCache = Arc::new(RwLock::new(None));

        let out_tx_srv = out_tx.clone();
        let config_cache_srv = Arc::clone(&config_cache);

        tokio::spawn(async move {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any);

            let app = Router::new()
                .route("/ws", get(ws_handler))
                .route("/health", get(|| async { "ok" }))
                .with_state((out_tx_srv, in_tx, config_cache_srv))
                .layer(cors);

            let addr = format!("0.0.0.0:{port}");
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("bind metrics WS port");
            println!("[ml-bench] ✓ Dashboard WS ready → ws://0.0.0.0:{port}/ws");
            axum::serve(listener, app).await.expect("metrics WS serve");
        });

        (out_tx, in_rx, config_cache)
    }

    async fn ws_handler(
        ws: WebSocketUpgrade,
        State((out_tx, in_tx, config_cache)): State<(
            broadcast::Sender<String>,
            broadcast::Sender<String>,
            ConfigCache,
        )>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_socket(socket, out_tx, in_tx, config_cache))
    }

    async fn handle_socket(
        mut socket: WebSocket,
        out_tx: broadcast::Sender<String>,
        _in_tx: broadcast::Sender<String>,
        config_cache: ConfigCache,
    ) {
        // Replay cached config to new client
        if let Some(cfg) = config_cache.read().await.clone() {
            if socket.send(Message::Text(cfg.into())).await.is_err() {
                return;
            }
        }

        let mut out_rx = out_tx.subscribe();
        loop {
            tokio::select! {
                // Bench → dashboard
                result = out_rx.recv() => {
                    match result {
                        Ok(msg) => {
                            if socket.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                    }
                }
                // Dashboard → bench (future: control commands like pause/resume)
                result = socket.recv() => {
                    match result {
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }
            }
        }
    }
}

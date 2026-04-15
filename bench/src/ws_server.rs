//! Embedded WebSocket metrics server for the Blazil bench dashboard.
//!
//! When the bench binary is invoked with `--metrics-port PORT`, this module
//! starts a lightweight Axum WebSocket server that broadcasts real-time
//! per-second metrics to all connected dashboard clients.
//!
//! # Usage
//!
//! ```bash
//! BLAZIL_TB_ADDRESS=... ./blazil-bench \
//!   --scenario sharded-tb \
//!   --shards 8 \
//!   --duration 600 \
//!   --metrics-port 9090
//! ```
//!
//! Then open the dashboard and connect to ws://<host>:9090/ws
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
    use tokio::sync::broadcast;
    use tower_http::cors::{Any, CorsLayer};

    /// Start the WS metrics server on `port`.
    ///
    /// Returns a `broadcast::Sender<String>` that the bench threads use to
    /// publish JSON metric lines. Every connected WebSocket subscriber
    /// receives all messages in real-time.
    pub fn start(port: u16) -> broadcast::Sender<String> {
        let (tx, _) = broadcast::channel::<String>(8192);
        let tx_srv = tx.clone();

        tokio::spawn(async move {
            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any);

            let app = Router::new()
                .route("/ws", get(ws_handler))
                .route("/health", get(|| async { "ok" }))
                .with_state(tx_srv)
                .layer(cors);

            let addr = format!("0.0.0.0:{port}");
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("bind metrics WS port");
            println!("[metrics-ws] ✓ Dashboard WS ready → ws://0.0.0.0:{port}/ws");
            axum::serve(listener, app).await.expect("metrics WS serve");
        });

        tx
    }

    async fn ws_handler(
        ws: WebSocketUpgrade,
        State(tx): State<broadcast::Sender<String>>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_socket(socket, tx))
    }

    async fn handle_socket(mut socket: WebSocket, tx: broadcast::Sender<String>) {
        let mut rx = tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                // Slow client: skip dropped frames rather than disconnect.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    }
}

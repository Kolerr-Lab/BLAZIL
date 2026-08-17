use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::config::Config;

#[derive(Clone)]
#[allow(dead_code)]
struct AppState {
    config: Arc<Config>,
}

pub async fn serve(config: Config) -> Result<()> {
    let bind_addr = config.bind_addr.clone();
    let state = AppState {
        config: Arc::new(config),
    };

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(metrics))
        .route("/media/twilio", get(twilio_ws_handler))
        .with_state(state);

    let listener = TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> impl IntoResponse {
    StatusCode::OK
}

async fn metrics() -> impl IntoResponse {
    // Return empty metrics for now
    (StatusCode::OK, "metrics")
}

async fn twilio_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_twilio_socket(socket, state))
}

async fn handle_twilio_socket(mut socket: WebSocket, _state: AppState) {
    tracing::info!("New Twilio WebSocket connection established");
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                tracing::debug!("Received text: {}", text);
            }
            Ok(Message::Close(_)) => {
                tracing::info!("Twilio WebSocket closed");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!("WebSocket error: {}", e);
                break;
            }
        }
    }
}

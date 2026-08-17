use anyhow::Result;
use axum::{
    extract::{ws::WebSocketUpgrade, State},
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
    ws.on_upgrade(|socket| async move {
        tracing::info!("New Twilio WebSocket connection established");
        let session = crate::session::Session::new(state.config.clone());
        session.run(socket).await;
    })
}

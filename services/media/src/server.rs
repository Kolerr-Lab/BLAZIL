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
use crate::turn_detector::SmartTurn;

#[derive(Clone)]
#[allow(dead_code)]
struct AppState {
    config: Arc<Config>,
    // Smart Turn is loaded ONCE at startup and shared across every call (Arc) instead of being
    // loaded per call in the Session's Start handler — that per-call ONNX load blocked setup and
    // spiked media-plane CPU. None when predictive endpointing is off or the model failed to load.
    smart_turn: Option<Arc<SmartTurn>>,
}

pub async fn serve(config: Config) -> Result<()> {
    let bind_addr = config.bind_addr.clone();
    let config = Arc::new(config);

    let smart_turn = if config.predictive_endpoint {
        match SmartTurn::load(&config.smart_turn_model_path, config.smart_turn_threshold) {
            Ok(st) => {
                tracing::info!("Smart Turn loaded once at startup (shared across all calls)");
                Some(Arc::new(st))
            }
            Err(e) => {
                tracing::error!("Smart Turn load failed at boot, using VAD endpointing: {e}");
                None
            }
        }
    } else {
        None
    };

    let state = AppState { config, smart_turn };

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
        let session = crate::session::Session::new(state.config.clone(), state.smart_turn.clone());
        session.run(socket).await;
    })
}

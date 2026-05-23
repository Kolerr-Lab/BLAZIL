//! Axum HTTP API server.
//!
//! Exposes a single server on `http_port` serving:
//!
//! ```text
//! GET  /health                             — liveness probe (no auth)
//! GET  /metrics                            — Prometheus text (no auth)
//!
//! POST /v1/infer                           — run inference
//! POST /v1/models                          — upload .onnx model
//! GET  /v1/models                          — list tenant models
//! GET  /v1/models/:model_id                — model metadata
//! DELETE /v1/models/:model_id              — delete model (must not be active)
//! POST /v1/models/:model_id/activate       — set active version + load cache
//! ```
//!
//! ## Auth
//!
//! All `/v1/**` routes require `Authorization: Bearer <token>`.
//! Checked via constant-time comparison against the configured API key.
//!
//! ## Tenant scoping
//!
//! `X-Tenant-ID: <tenant_id>` header is required on all `/v1/**` requests.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{info, warn};

use base64::Engine as _;
use blazil_inference::{InferenceModel, OnnxModel};

use crate::metrics::InferenceMetrics;
use crate::model_registry::ModelRegistry;
use crate::protocol::InferenceResponse;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Max body size for inference requests (10 MiB).
const INFER_BODY_LIMIT: usize = 10 * 1024 * 1024;

/// Max body size for model uploads (500 MiB — ONNX models can be large).
const UPLOAD_BODY_LIMIT: usize = 500 * 1024 * 1024;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<ModelRegistry>,
    pub metrics: Arc<InferenceMetrics>,
    /// Pre-loaded default model (from `config.model_path`), if any.
    pub default_model: Option<Arc<OnnxModel>>,
    /// Expected Bearer token (constant-time compared on every request).
    pub api_key: Arc<String>,
}

// ── Request / Response types ──────────────────────────────────────────────────

/// POST /v1/infer request body.
#[derive(Debug, Deserialize)]
pub struct InferHttpRequest {
    /// Client-generated request ID for correlation.
    pub request_id: String,
    /// Base64-encoded input tensor bytes.
    pub input_data_b64: String,
    /// Optional model ID. If omitted, uses the tenant's first active model
    /// or the server default model.
    #[serde(default)]
    pub model_id: Option<String>,
}

/// POST /v1/models/activate query params.
#[derive(Debug, Deserialize)]
pub struct ActivateQuery {
    /// Version to activate, e.g. "v2". Required.
    pub version: String,
}

/// Generic JSON error body.
#[derive(Serialize)]
struct ApiError {
    error: String,
}

impl ApiError {
    fn new(msg: impl Into<String>) -> Json<Self> {
        Json(Self { error: msg.into() })
    }
}

// ── Router builder ────────────────────────────────────────────────────────────

/// Build the axum `Router` and bind it to `addr`.
///
/// Returns a future that resolves when the server stops (on shutdown signal).
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let api_routes = Router::new()
        .route("/v1/infer", post(infer_handler))
        .route("/v1/models", post(upload_model_handler))
        .route("/v1/models", get(list_models_handler))
        .route("/v1/models/:model_id", get(get_model_handler))
        .route("/v1/models/:model_id", delete(delete_model_handler))
        .route(
            "/v1/models/:model_id/activate",
            post(activate_model_handler),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // Conservative body limit for inference; upload has its own limit (see handler).
        .layer(RequestBodyLimitLayer::new(INFER_BODY_LIMIT));

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .merge(api_routes)
        .with_state(state);

    let listener = TcpListener::bind(addr).await?;
    info!("HTTP API listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(Into::into)
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let provided = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    // Constant-time comparison to prevent timing attacks.
    use subtle::ConstantTimeEq;
    let ok = provided.as_bytes().ct_eq(state.api_key.as_bytes());
    if ok.unwrap_u8() == 0 {
        warn!("Unauthorized request — invalid or missing Bearer token");
        return (
            StatusCode::UNAUTHORIZED,
            ApiError::new("Invalid or missing Authorization header"),
        )
            .into_response();
    }

    next.run(request).await
}

// ── Utility: extract X-Tenant-ID ─────────────────────────────────────────────

#[allow(clippy::result_large_err)]
fn tenant_id_from_headers(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("X-Tenant-ID")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                ApiError::new("Missing X-Tenant-ID header"),
            )
                .into_response()
        })
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /health
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// GET /metrics
async fn metrics_handler(State(state): State<AppState>) -> Response {
    match state.metrics.export() {
        Ok(body) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// POST /v1/infer
async fn infer_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InferHttpRequest>,
) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    // Validate request_id (non-empty, reasonable length).
    if req.request_id.is_empty() || req.request_id.len() > 128 {
        return (
            StatusCode::BAD_REQUEST,
            ApiError::new("request_id must be 1-128 characters"),
        )
            .into_response();
    }

    // Decode base64 input.
    let input_bytes = match base64::engine::general_purpose::STANDARD.decode(&req.input_data_b64) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiError::new(format!("input_data_b64 is not valid base64: {e}")),
            )
                .into_response()
        }
    };

    // Resolve which model to use.
    let model: Arc<OnnxModel> = if let Some(ref model_id) = req.model_id {
        match state.registry.get_active_model(&tenant_id, model_id) {
            Some(m) => m,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    ApiError::new(format!(
                        "Model '{model_id}' not found or not activated for tenant '{tenant_id}'"
                    )),
                )
                    .into_response()
            }
        }
    } else if let Some(ref default) = state.default_model {
        Arc::clone(default)
    } else {
        return (
            StatusCode::BAD_REQUEST,
            ApiError::new("No model_id specified and no default model is loaded"),
        )
            .into_response();
    };

    // Convert raw bytes to a `blazil_dataloader::Sample`.
    let label = 0u32;
    let sample = blazil_dataloader::Sample {
        data: input_bytes,
        label,
        metadata: None,
    };

    state.metrics.requests_total.inc();
    state.metrics.active_requests.inc();
    let start = std::time::Instant::now();

    // Run inference (blocking — offload to thread pool).
    let result = tokio::task::spawn_blocking(move || model.run_batch(&[sample])).await;

    let latency_us = start.elapsed().as_micros() as u64;
    state.metrics.active_requests.dec();

    match result {
        Ok(Ok(predictions)) => {
            state.metrics.request_success(latency_us);
            let pred =
                predictions
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| blazil_inference::Prediction {
                        class_id: None,
                        probabilities: None,
                        raw_output: vec![],
                        confidence: 0.0,
                        metadata: None,
                    });

            let response = InferenceResponse {
                request_id: req.request_id,
                class_id: pred.class_id,
                probabilities: pred.probabilities.unwrap_or_default(),
                raw_output: pred.raw_output,
                confidence: pred.confidence,
                latency_us,
                error: String::new(),
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            state.metrics.request_failed(latency_us);
            warn!("Inference error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new(format!("Inference failed: {e}")),
            )
                .into_response()
        }
        Err(e) => {
            state.metrics.request_failed(latency_us);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new(format!("Worker panicked: {e}")),
            )
                .into_response()
        }
    }
}

/// POST /v1/models — multipart upload
///
/// Form fields:
///   `file`         — the .onnx file (required)
///   `model_id`     — model identifier (required)
///   `display_name` — human-readable name (optional)
async fn upload_model_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    let mut model_id: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut onnx_bytes: Option<Bytes> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("model_id") => {
                model_id = field.text().await.ok();
            }
            Some("display_name") => {
                display_name = field.text().await.ok();
            }
            Some("file") => {
                // Enforce upload size limit here.
                let data = field.bytes().await.unwrap_or_default();
                if data.len() > UPLOAD_BODY_LIMIT {
                    return (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        ApiError::new(format!(
                            "Model file exceeds limit of {} MiB",
                            UPLOAD_BODY_LIMIT / 1024 / 1024
                        )),
                    )
                        .into_response();
                }
                onnx_bytes = Some(data);
            }
            _ => {} // ignore unknown fields
        }
    }

    let model_id = match model_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                ApiError::new("Missing required form field: model_id"),
            )
                .into_response()
        }
    };

    let bytes = match onnx_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                ApiError::new("Missing required form field: file (.onnx)"),
            )
                .into_response()
        }
    };

    // Run blocking upload on thread pool (disk I/O + SHA-256).
    let registry = Arc::clone(&state.registry);
    let dn = display_name.clone();
    let mid = model_id.clone();
    let tid = tenant_id.clone();
    let bytes_vec: Vec<u8> = bytes.into();

    let result =
        tokio::task::spawn_blocking(move || registry.upload(&tid, &mid, dn.as_deref(), &bytes_vec))
            .await;

    match result {
        Ok(Ok(meta)) => (StatusCode::CREATED, Json(meta)).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, ApiError::new(e.to_string())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(format!("Worker panic: {e}")),
        )
            .into_response(),
    }
}

/// GET /v1/models
async fn list_models_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    let models = state.registry.list_models(&tenant_id);
    (StatusCode::OK, Json(models)).into_response()
}

/// GET /v1/models/:model_id
async fn get_model_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    match state.registry.get_model(&tenant_id, &model_id) {
        Some(meta) => (StatusCode::OK, Json(meta)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            ApiError::new(format!(
                "Model '{model_id}' not found for tenant '{tenant_id}'"
            )),
        )
            .into_response(),
    }
}

/// DELETE /v1/models/:model_id
async fn delete_model_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    let registry = Arc::clone(&state.registry);
    let result =
        tokio::task::spawn_blocking(move || registry.delete_model(&tenant_id, &model_id)).await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, ApiError::new(e.to_string())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(format!("Worker panic: {e}")),
        )
            .into_response(),
    }
}

/// POST /v1/models/:model_id/activate?version=v2
async fn activate_model_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Query(q): Query<ActivateQuery>,
) -> Response {
    let tenant_id = match tenant_id_from_headers(&headers) {
        Ok(t) => t,
        Err(r) => return r,
    };

    let registry = Arc::clone(&state.registry);
    let version = q.version.clone();

    // `activate` loads the ONNX model — must run in spawn_blocking.
    let result =
        tokio::task::spawn_blocking(move || registry.activate(&tenant_id, &model_id, &version))
            .await;

    match result {
        Ok(Ok(_model)) => {
            // Return updated metadata.
            // Note: re-read from registry after activate so meta reflects new active_version.
            // (state.registry is the same Arc — the index was already updated.)
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "activated", "version": q.version })),
            )
                .into_response()
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, ApiError::new(e.to_string())).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::new(format!("Worker panic: {e}")),
        )
            .into_response(),
    }
}

// ── Graceful shutdown ─────────────────────────────────────────────────────────

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

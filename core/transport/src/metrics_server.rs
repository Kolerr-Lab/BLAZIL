//! HTTP metrics endpoint for the Blazil transport/engine layer.
//!
//! Exposes Prometheus-compatible text on `GET /metrics` using a lightweight
//! axum server. The server reads from an `Arc<EngineMetrics>` so there is
//! zero contention with the hot transaction path.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use blazil_engine::metrics::EngineMetrics;
//! use blazil_transport::metrics_server::MetricsServer;
//!
//! #[tokio::main]
//! async fn main() {
//!     let m = EngineMetrics::new();
//!     let srv = MetricsServer::new(m, "0.0.0.0:9090".to_string());
//!     srv.serve().await;
//! }
//! ```

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use blazil_engine::metrics::EngineMetrics;
use tokio::net::TcpListener;

/// Lightweight HTTP server that exposes engine metrics in Prometheus text format.
pub struct MetricsServer {
    metrics: Arc<EngineMetrics>,
    addr: String,
}

impl MetricsServer {
    /// Creates a new `MetricsServer`.
    ///
    /// * `metrics` — shared reference to the engine's atomic counters.
    /// * `addr` — bind address, e.g. `"0.0.0.0:9090"`.
    pub fn new(metrics: Arc<EngineMetrics>, addr: String) -> Self {
        Self { metrics, addr }
    }

    /// Binds to `self.addr` and serves requests until the process exits.
    ///
    /// Should be called inside a `tokio::spawn`.
    pub async fn serve(self) {
        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(self.metrics);

        let listener = TcpListener::bind(&self.addr)
            .await
            .expect("metrics: failed to bind");

        axum::serve(listener, app)
            .await
            .expect("metrics: server error");
    }
}

/// Handler that serialises `EngineMetrics` into Prometheus text exposition format.
async fn metrics_handler(State(m): State<Arc<EngineMetrics>>) -> impl IntoResponse {
    let body = format!(
        "# HELP blazil_pipeline_events_published_total Events published to the ring buffer\n\
# TYPE blazil_pipeline_events_published_total counter\n\
blazil_pipeline_events_published_total {published}\n\
# HELP blazil_pipeline_events_committed_total Events committed to the ledger\n\
# TYPE blazil_pipeline_events_committed_total counter\n\
blazil_pipeline_events_committed_total {committed}\n\
# HELP blazil_pipeline_events_rejected_total Events rejected by the pipeline\n\
# TYPE blazil_pipeline_events_rejected_total counter\n\
blazil_pipeline_events_rejected_total {rejected}\n\
# HELP blazil_pipeline_avg_latency_ns Average commit latency in nanoseconds\n\
# TYPE blazil_pipeline_avg_latency_ns gauge\n\
blazil_pipeline_avg_latency_ns {avg_ns}\n\
# HELP blazil_pipeline_p99_ns Peak (proxy for p99) commit latency in nanoseconds\n\
# TYPE blazil_pipeline_p99_ns gauge\n\
blazil_pipeline_p99_ns {p99_ns}\n\
# HELP blazil_ring_buffer_utilization_ratio Ring buffer utilization (0.0–1.0)\n\
# TYPE blazil_ring_buffer_utilization_ratio gauge\n\
blazil_ring_buffer_utilization_ratio{{instance=\"engine\"}} {util}\n",
        published = m.published(),
        committed = m.committed(),
        rejected = m.rejected(),
        avg_ns = m.avg_latency_ns(),
        p99_ns = m.peak_latency_ns(),
        util = m.ring_utilization_x10000() as f64 / 10_000.0,
    );

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::util::ServiceExt; // for `oneshot`

    fn build_app(m: Arc<EngineMetrics>) -> Router {
        Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(m)
    }

    #[tokio::test]
    async fn test_metrics_endpoint_returns_200() {
        let m = EngineMetrics::new();
        let app = build_app(m);

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_metrics_contains_required_fields() {
        let m = EngineMetrics::new();
        m.record_published();
        m.record_committed(1_500_000);
        m.record_rejected();

        let app = build_app(m);

        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();

        assert!(
            text.contains("blazil_pipeline_events_published_total"),
            "missing published counter"
        );
        assert!(
            text.contains("blazil_pipeline_events_committed_total"),
            "missing committed counter"
        );
        assert!(
            text.contains("blazil_pipeline_events_rejected_total"),
            "missing rejected counter"
        );
        assert!(text.contains("blazil_pipeline_p99_ns"), "missing p99 gauge");
        assert!(
            text.contains("blazil_ring_buffer_utilization_ratio"),
            "missing utilization gauge"
        );
    }
}

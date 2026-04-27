//! Prometheus metrics for inference server.

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntGauge, Registry, TextEncoder,
};

/// Inference server metrics.
#[allow(dead_code)]
pub struct InferenceMetrics {
    registry: Registry,

    /// Total inference requests received.
    pub requests_total: IntCounter,

    /// Total successful inferences.
    pub requests_success_total: IntCounter,

    /// Total failed inferences.
    pub requests_failed_total: IntCounter,

    /// Total predictions generated.
    pub predictions_total: IntCounter,

    /// Request latency histogram (microseconds).
    pub request_latency_us: HistogramVec,

    /// Currently active inference requests.
    pub active_requests: IntGauge,

    /// Aeron offer failures (backpressure).
    pub aeron_offer_failures: IntCounter,
}

impl InferenceMetrics {
    /// Create a new metrics registry.
    pub fn new() -> anyhow::Result<Self> {
        let registry = Registry::new();

        let requests_total = IntCounter::new(
            "inference_requests_total",
            "Total inference requests received",
        )?;
        registry.register(Box::new(requests_total.clone()))?;

        let requests_success_total = IntCounter::new(
            "inference_requests_success_total",
            "Total successful inference requests",
        )?;
        registry.register(Box::new(requests_success_total.clone()))?;

        let requests_failed_total = IntCounter::new(
            "inference_requests_failed_total",
            "Total failed inference requests",
        )?;
        registry.register(Box::new(requests_failed_total.clone()))?;

        let predictions_total =
            IntCounter::new("inference_predictions_total", "Total predictions generated")?;
        registry.register(Box::new(predictions_total.clone()))?;

        let request_latency_us = HistogramVec::new(
            HistogramOpts::new(
                "inference_request_latency_microseconds",
                "Inference request latency in microseconds",
            )
            .buckets(vec![
                100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0, 25000.0, 50000.0,
            ]),
            &["status"],
        )?;
        registry.register(Box::new(request_latency_us.clone()))?;

        let active_requests = IntGauge::new(
            "inference_active_requests",
            "Currently active inference requests",
        )?;
        registry.register(Box::new(active_requests.clone()))?;

        let aeron_offer_failures = IntCounter::new(
            "inference_aeron_offer_failures_total",
            "Total Aeron offer() failures (backpressure)",
        )?;
        registry.register(Box::new(aeron_offer_failures.clone()))?;

        Ok(Self {
            registry,
            requests_total,
            requests_success_total,
            requests_failed_total,
            predictions_total,
            request_latency_us,
            active_requests,
            aeron_offer_failures,
        })
    }

    /// Export metrics in Prometheus text format.
    pub fn export(&self) -> anyhow::Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .map_err(|e| anyhow::anyhow!("encode metrics: {e}"))?;
        String::from_utf8(buffer).map_err(|e| anyhow::anyhow!("utf8 error: {e}"))
    }

    /// Record a successful request.
    #[allow(dead_code)]
    pub fn request_success(&self, latency_us: u64) {
        self.requests_total.inc();
        self.requests_success_total.inc();
        self.request_latency_us
            .with_label_values(&["success"])
            .observe(latency_us as f64);
    }

    /// Record a failed request.
    #[allow(dead_code)]
    pub fn request_failed(&self, latency_us: u64) {
        self.requests_total.inc();
        self.requests_failed_total.inc();
        self.request_latency_us
            .with_label_values(&["error"])
            .observe(latency_us as f64);
    }
}

impl Default for InferenceMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create metrics")
    }
}

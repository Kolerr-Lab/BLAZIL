// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Service health and SLA metrics tracking.
//!
//! Tracks uptime, request success rate, latency percentiles, and exposes
//! /health and /metrics endpoints for production monitoring.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// Service health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// All systems operational
    Healthy,
    /// Degraded performance (high error rate, slow latency)
    Degraded,
    /// Critical failure (model not loaded, OOM, etc.)
    Unhealthy,
}

impl HealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }

    pub fn http_code(&self) -> u16 {
        match self {
            Self::Healthy => 200,
            Self::Degraded => 200, // still serving traffic
            Self::Unhealthy => 503,
        }
    }
}

/// SLA compliance thresholds
pub struct SlaConfig {
    /// Maximum acceptable error rate (0.0–1.0)
    pub max_error_rate: f64,
    /// Maximum acceptable P99 latency (microseconds)
    pub max_p99_latency_us: u64,
    /// Minimum required uptime percentage (0.0–1.0)
    pub min_uptime_pct: f64,
}

impl Default for SlaConfig {
    fn default() -> Self {
        Self {
            max_error_rate: 0.01,       // 1% max error rate
            max_p99_latency_us: 50_000, // 50ms P99
            min_uptime_pct: 0.999,      // 99.9% uptime
        }
    }
}

/// Global service health tracker
pub struct HealthTracker {
    /// Service start time
    start_time: Instant,
    start_unix: u64,

    /// Model loading state
    model_loaded: AtomicBool,

    /// Total requests processed
    total_requests: AtomicU64,
    /// Total successful requests
    successful_requests: AtomicU64,
    /// Total failed requests
    failed_requests: AtomicU64,

    /// Rolling latencies (last 1000 samples)
    latencies_us: Mutex<Vec<u64>>,

    /// Fault injection active
    fault_active: AtomicBool,

    /// SLA configuration
    sla_config: SlaConfig,
}

impl HealthTracker {
    pub fn new(sla_config: SlaConfig) -> Arc<Self> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Arc::new(Self {
            start_time: Instant::now(),
            start_unix: now,
            model_loaded: AtomicBool::new(false),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            latencies_us: Mutex::new(Vec::with_capacity(1000)),
            fault_active: AtomicBool::new(false),
            sla_config,
        })
    }

    /// Mark model as loaded
    pub fn set_model_loaded(&self, loaded: bool) {
        self.model_loaded.store(loaded, Ordering::SeqCst);
    }

    /// Record a successful request
    pub fn record_success(&self, latency_us: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);

        // Keep rolling window of last 1000 latencies
        let mut lats = self.latencies_us.lock().unwrap();
        lats.push(latency_us);
        if lats.len() > 1000 {
            lats.remove(0);
        }
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark fault injection active/inactive
    pub fn set_fault_active(&self, active: bool) {
        self.fault_active.store(active, Ordering::SeqCst);
    }

    /// Calculate current health status
    pub fn status(&self) -> HealthStatus {
        // Critical: model not loaded
        if !self.model_loaded.load(Ordering::Relaxed) {
            return HealthStatus::Unhealthy;
        }

        // Critical: fault injection active
        if self.fault_active.load(Ordering::Relaxed) {
            return HealthStatus::Degraded;
        }

        let total = self.total_requests.load(Ordering::Relaxed);
        if total < 10 {
            // Not enough data yet
            return HealthStatus::Healthy;
        }

        // Check error rate
        let failures = self.failed_requests.load(Ordering::Relaxed);
        let error_rate = failures as f64 / total as f64;
        if error_rate > self.sla_config.max_error_rate {
            return HealthStatus::Degraded;
        }

        // Check P99 latency
        let p99 = self.p99_latency_us();
        if p99 > self.sla_config.max_p99_latency_us {
            return HealthStatus::Degraded;
        }

        HealthStatus::Healthy
    }

    /// Get uptime in seconds
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Get uptime as ISO8601 duration string
    pub fn uptime_iso8601(&self) -> String {
        let secs = self.uptime_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let sec = secs % 60;
        format!("PT{hours}H{mins}M{sec}S")
    }

    /// Get success rate (0.0–1.0)
    pub fn success_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 {
            return 1.0;
        }
        let success = self.successful_requests.load(Ordering::Relaxed);
        success as f64 / total as f64
    }

    /// Get error rate (0.0–1.0)
    pub fn error_rate(&self) -> f64 {
        1.0 - self.success_rate()
    }

    /// Get P50 latency in microseconds
    pub fn p50_latency_us(&self) -> u64 {
        self.percentile_latency(50.0)
    }

    /// Get P99 latency in microseconds
    pub fn p99_latency_us(&self) -> u64 {
        self.percentile_latency(99.0)
    }

    /// Get P999 latency in microseconds
    pub fn p999_latency_us(&self) -> u64 {
        self.percentile_latency(99.9)
    }

    /// Calculate percentile from rolling latencies
    fn percentile_latency(&self, pct: f64) -> u64 {
        let lats = self.latencies_us.lock().unwrap();
        if lats.is_empty() {
            return 0;
        }
        let mut sorted = lats.clone();
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64 * pct / 100.0) as usize).min(sorted.len() - 1);
        sorted[idx]
    }

    /// Check if service meets SLA requirements
    pub fn meets_sla(&self) -> bool {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total < 100 {
            // Not enough data yet
            return true;
        }

        // Check error rate
        if self.error_rate() > self.sla_config.max_error_rate {
            return false;
        }

        // Check P99 latency
        if self.p99_latency_us() > self.sla_config.max_p99_latency_us {
            return false;
        }

        // Uptime check: assume we count start time as uptime
        // In production, track downtime separately
        let uptime_pct = self.success_rate();
        if uptime_pct < self.sla_config.min_uptime_pct {
            return false;
        }

        true
    }

    /// Generate health check JSON response
    pub fn health_json(&self) -> serde_json::Value {
        let status = self.status();
        serde_json::json!({
            "status": status.as_str(),
            "uptime_secs": self.uptime_secs(),
            "uptime": self.uptime_iso8601(),
            "start_time_unix": self.start_unix,
            "model_loaded": self.model_loaded.load(Ordering::Relaxed),
            "total_requests": self.total_requests.load(Ordering::Relaxed),
            "successful_requests": self.successful_requests.load(Ordering::Relaxed),
            "failed_requests": self.failed_requests.load(Ordering::Relaxed),
            "success_rate": format!("{:.4}", self.success_rate()),
            "error_rate": format!("{:.4}", self.error_rate()),
            "latency": {
                "p50_us": self.p50_latency_us(),
                "p99_us": self.p99_latency_us(),
                "p999_us": self.p999_latency_us(),
            },
            "sla": {
                "meets_sla": self.meets_sla(),
                "max_error_rate": self.sla_config.max_error_rate,
                "max_p99_latency_us": self.sla_config.max_p99_latency_us,
                "min_uptime_pct": self.sla_config.min_uptime_pct,
            },
            "fault_injection_active": self.fault_active.load(Ordering::Relaxed),
        })
    }

    /// Generate Prometheus-compatible metrics text
    pub fn metrics_text(&self) -> String {
        let uptime = self.uptime_secs();
        let _total = self.total_requests.load(Ordering::Relaxed);
        let success = self.successful_requests.load(Ordering::Relaxed);
        let failures = self.failed_requests.load(Ordering::Relaxed);
        let p50 = self.p50_latency_us();
        let p99 = self.p99_latency_us();
        let p999 = self.p999_latency_us();
        let status_code = self.status().http_code();

        format!(
            r#"# HELP ml_bench_uptime_seconds Service uptime in seconds
# TYPE ml_bench_uptime_seconds gauge
ml_bench_uptime_seconds {uptime}

# HELP ml_bench_requests_total Total requests processed
# TYPE ml_bench_requests_total counter
ml_bench_requests_total{{result="success"}} {success}
ml_bench_requests_total{{result="failure"}} {failures}

# HELP ml_bench_latency_microseconds Request latency percentiles
# TYPE ml_bench_latency_microseconds gauge
ml_bench_latency_microseconds{{quantile="0.5"}} {p50}
ml_bench_latency_microseconds{{quantile="0.99"}} {p99}
ml_bench_latency_microseconds{{quantile="0.999"}} {p999}

# HELP ml_bench_health_status Health status HTTP code (200=healthy, 503=unhealthy)
# TYPE ml_bench_health_status gauge
ml_bench_health_status {status_code}

# HELP ml_bench_sla_compliance SLA compliance (1=compliant, 0=non-compliant)
# TYPE ml_bench_sla_compliance gauge
ml_bench_sla_compliance {}
"#,
            if self.meets_sla() { 1 } else { 0 }
        )
    }
}

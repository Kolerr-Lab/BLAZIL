//! Library exports for inference service.

pub mod config;
pub mod metrics;
pub mod protocol;
pub mod server;

// Re-exports
pub use config::ServerConfig;
pub use metrics::InferenceMetrics;
pub use protocol::{
    InferenceRequest, InferenceResponse, INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
};
pub use server::{AeronInferenceServer, DEFAULT_INFERENCE_CHANNEL};

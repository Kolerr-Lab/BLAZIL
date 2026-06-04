//! Library exports for inference service.

pub mod config;
pub mod gguf_model;
pub mod http_api;
pub mod metrics;
pub mod model_registry;
pub mod protocol;
pub mod server;

// Re-exports
pub use config::{GgufConfig, ModelBackend, ServerConfig};
pub use gguf_model::GgufModel;
pub use http_api::AppState;
pub use metrics::InferenceMetrics;
pub use model_registry::ModelRegistry;
pub use protocol::{
    InferenceRequest, InferenceResponse, INFERENCE_REQ_STREAM_ID, INFERENCE_RSP_STREAM_ID,
};
pub use server::{AeronInferenceServer, DEFAULT_INFERENCE_CHANNEL};

//! Vendored model architectures with distributed pipeline support.
//!
//! This module contains locally vendored versions of Candle model architectures
//! modified to support true layer-wise execution for distributed inference pipelines.
//!
//! ## Why Vendored?
//!
//! Candle's upstream `ModelWeights` struct keeps `layers: Vec<LayerWeights>` private,
//! preventing direct layer-by-layer execution required for pipeline parallelism.
//!
//! By vendoring the code locally, we gain full control to:
//! - Expose `layers` field as `pub`
//! - Implement `forward_layer_range(layer_start, layer_end)` for true mathematical slicing
//! - Support multi-stage activation tensor transfers via Aeron IPC
//!
//! ## Modifications from Upstream
//!
//! 1. **`ModelWeights.layers`**: Changed from private to `pub`
//! 2. **`forward_layer_range()`**: New method for partial layer execution
//! 3. **KV Cache Locality**: Maintained per-stage (never transferred)
//! 4. **AVX-512 VNNI Kernels**: Production SIMD optimizations for RmsNorm, Attention, MLP

pub mod quantized_qwen2;

#[cfg(target_arch = "x86_64")]
pub mod avx512_kernels;

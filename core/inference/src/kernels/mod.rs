// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! High-performance compute kernels for model inference.
//!
//! This module provides optimized implementations for common operations:
//! - BitNet 1-bit linear layers (extreme quantization)
//! - INT8 quantization/dequantization
//! - AVX-512 SIMD paths with scalar fallbacks

pub mod bitnet;

pub use bitnet::{
    bitnet_linear_1bit, dequantize_int8, pack_weights_1bit, quantize_int8, KernelError,
};

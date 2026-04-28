// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Dataset implementations for common ML formats.

pub mod imagenet;
pub mod text;
pub mod timeseries;
pub mod features;
pub mod audio;
pub mod detection;

// Re-exports
pub use imagenet::ImageNetDataset;
pub use text::TextDataset;
pub use timeseries::TimeSeriesDataset;
pub use features::FeatureDataset;
pub use audio::AudioDataset;
pub use detection::{DetectionDataset, BoundingBox, ImageAnnotation};

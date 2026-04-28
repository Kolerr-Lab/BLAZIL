// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Dataset implementations for common ML formats.

pub mod audio;
pub mod detection;
pub mod features;
pub mod imagenet;
pub mod text;
pub mod timeseries;

// Re-exports
pub use audio::AudioDataset;
pub use detection::{BoundingBox, DetectionDataset, ImageAnnotation};
pub use features::FeatureDataset;
pub use imagenet::ImageNetDataset;
pub use text::TextDataset;
pub use timeseries::TimeSeriesDataset;

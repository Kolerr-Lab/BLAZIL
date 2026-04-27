// Copyright (c) 2026 Blazil Contributors
// SPDX-License-Identifier: BSL-1.1

//! Inference pipeline — connects dataloader batches to model inference.

use crate::{model::InferenceModel, Result};
use blazil_dataloader::Batch;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Inference result for a batch.
#[derive(Debug)]
pub struct InferenceBatch {
    /// Original batch ID from the dataloader.
    pub batch_id: u64,
    /// Predictions for each sample in the batch.
    pub predictions: Vec<crate::model::Prediction>,
}

/// Inference pipeline — consumes batches from a dataloader and runs inference.
///
/// # Architecture
/// ```text
///   DataPipeline (dataloader)
///         │
///         ├─ Batch stream
///         ▼
///   InferencePipeline::stream()
///         │
///         ├─ spawn_blocking(model.run_batch)
///         ▼
///   mpsc::Receiver<InferenceBatch>  ← consumer (training loop / API)
/// ```
pub struct InferencePipeline<M: InferenceModel> {
    model: Arc<M>,
    num_workers: usize,
}

impl<M: InferenceModel + 'static> InferencePipeline<M> {
    /// Create a new inference pipeline with the given model.
    ///
    /// `num_workers` controls the number of concurrent inference tasks.
    /// Each task runs `model.run_batch()` in a blocking thread pool.
    pub fn new(model: M, num_workers: usize) -> Self {
        Self {
            model: Arc::new(model),
            num_workers,
        }
    }

    /// Start the inference pipeline.
    ///
    /// Consumes batches from `data_receiver` (from a dataloader `Pipeline`),
    /// runs inference on each batch using a worker pool, and returns a receiver
    /// for `InferenceBatch` results.
    ///
    /// **Architecture:**
    /// - Single coordinator task receives from `data_receiver`
    /// - Work items dispatched to a pool of `num_workers` inference workers
    /// - Each worker runs `model.run_batch()` in `spawn_blocking` (CPU/GPU bound)
    /// - Backpressure: work queue bounded to `num_workers * 2`
    ///
    /// The returned receiver will close when:
    /// - The input `data_receiver` closes (no more batches), OR
    /// - An inference error occurs (error is sent, then channel closes).
    ///
    /// # Example
    /// ```no_run
    /// use blazil_inference::{OnnxModel, InferencePipeline, InferenceConfig, InferenceModel};
    /// use blazil_dataloader::{datasets::ImageNetDataset, DatasetConfig, Dataset, Pipeline};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let dataset = ImageNetDataset::open("/data/imagenet", DatasetConfig::default())?;
    /// let data_pipeline = Pipeline::new(dataset, DatasetConfig::default());
    /// let data_rx = data_pipeline.stream();
    ///
    /// let model = OnnxModel::load(InferenceConfig::new("model.onnx"))?;
    /// let inference_pipeline = InferencePipeline::new(model, 4);
    /// let mut inference_rx = inference_pipeline.stream(data_rx).await?;
    ///
    /// while let Some(result) = inference_rx.recv().await {
    ///     match result {
    ///         Ok(batch) => println!("Batch {} predictions: {}", batch.batch_id, batch.predictions.len()),
    ///         Err(e) => eprintln!("Inference error: {e}"),
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(
        &self,
        mut data_receiver: mpsc::Receiver<blazil_dataloader::Result<Batch>>,
    ) -> Result<mpsc::Receiver<Result<InferenceBatch>>> {
        let (result_tx, result_rx) = mpsc::channel(self.num_workers * 2);
        let (work_tx, work_rx) = mpsc::channel::<Batch>(self.num_workers * 2);
        let work_rx = Arc::new(tokio::sync::Mutex::new(work_rx));

        // Coordinator task: receive from dataloader, dispatch to workers.
        let coord_tx = result_tx.clone();
        tokio::spawn(async move {
            while let Some(batch_result) = data_receiver.recv().await {
                match batch_result {
                    Ok(batch) => {
                        if work_tx.send(batch).await.is_err() {
                            break; // workers shut down
                        }
                    }
                    Err(e) => {
                        // Propagate dataloader error and stop.
                        let _ = coord_tx.send(Err(e.into())).await;
                        break;
                    }
                }
            }
            tracing::debug!("Inference coordinator stopped");
        });

        // Worker pool: process batches from work queue.
        for worker_id in 0..self.num_workers {
            let result_tx = result_tx.clone();
            let model = Arc::clone(&self.model);
            let work_rx = Arc::clone(&work_rx);

            tokio::spawn(async move {
                loop {
                    // Receive next batch from work queue (shared across workers).
                    let batch = {
                        let mut rx = work_rx.lock().await;
                        match rx.recv().await {
                            Some(b) => b,
                            None => break, // coordinator closed work queue
                        }
                    };

                    let batch_id = batch.batch_id;
                    let samples = batch.samples;

                    tracing::trace!(
                        worker_id = worker_id,
                        batch_id = batch_id,
                        batch_size = samples.len(),
                        "Running inference",
                    );

                    // Run inference in a blocking thread (CPU/CUDA compute-heavy).
                    let model_clone = Arc::clone(&model);
                    let inference_result =
                        tokio::task::spawn_blocking(move || model_clone.run_batch(&samples)).await;

                    let result = match inference_result {
                        Ok(Ok(predictions)) => Ok(InferenceBatch {
                            batch_id,
                            predictions,
                        }),
                        Ok(Err(e)) => Err(e),
                        Err(e) => Err(crate::Error::internal(format!("worker panicked: {e}"))),
                    };

                    // Send result to consumer.
                    if result_tx.send(result).await.is_err() {
                        break; // consumer dropped receiver
                    }
                }
                tracing::debug!(worker_id = worker_id, "Inference worker stopped");
            });
        }

        Ok(result_rx)
    }
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::InferenceConfig, model::Prediction};
    use blazil_dataloader::Sample;

    /// Fake model for testing the pipeline.
    struct FakeModel {
        config: InferenceConfig,
    }

    impl InferenceModel for FakeModel {
        fn load(config: InferenceConfig) -> Result<Self> {
            Ok(FakeModel { config })
        }

        fn run_batch(&self, samples: &[Sample]) -> Result<Vec<Prediction>> {
            Ok(samples
                .iter()
                .map(|s| Prediction::from_logits(vec![s.label as f32, 0.0]))
                .collect())
        }

        fn input_shape(&self) -> (usize, usize, usize, usize) {
            (1, 3, 224, 224)
        }

        fn num_classes(&self) -> Option<usize> {
            Some(2)
        }

        fn config(&self) -> &InferenceConfig {
            &self.config
        }
    }

    #[tokio::test]
    async fn test_inference_pipeline_processes_all_batches() {
        let (data_tx, data_rx) = mpsc::channel(10);

        // Send 3 batches.
        for i in 0..3 {
            let batch = Batch {
                batch_id: i,
                samples: vec![Sample {
                    data: vec![0u8; 224 * 224 * 3],
                    label: i as u32,
                    metadata: None,
                }],
            };
            data_tx.send(Ok(batch)).await.unwrap();
        }
        drop(data_tx); // close channel

        let model = FakeModel::load(InferenceConfig::default()).unwrap();
        let pipeline = InferencePipeline::new(model, 2);
        let mut inference_rx = pipeline.stream(data_rx).await.unwrap();

        let mut results = Vec::new();
        while let Some(result) = inference_rx.recv().await {
            results.push(result.unwrap());
        }

        assert_eq!(results.len(), 3);
        // All batches processed
        let mut ids: Vec<u64> = results.iter().map(|r| r.batch_id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![0, 1, 2]);
    }
}

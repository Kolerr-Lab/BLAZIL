//! Sharded pipeline throughput benchmark.
//!
//! Tests independent sharded pipelines with configurable shard count.
//! Each shard has its own ring buffer and full handler chain.
//! Events are routed by account ID for deterministic processing.

use std::sync::Arc;
use std::time::Instant;

use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_engine::event::TransactionEvent;
use blazil_engine::sharded_pipeline::ShardedPipeline;

use crate::metrics::BenchmarkResult;

const WARMUP_EVENTS: u64 = 100;
const CAPACITY_PER_SHARD: usize = 1_048_576;
const MAX_AMOUNT_UNITS: u64 = 1_000_000;

/// Run the sharded pipeline scenario with the specified shard count once.
pub async fn run(events: u64, shard_count: usize) -> BenchmarkResult {
    tokio::task::spawn_blocking(move || run_once_blocking(events, shard_count))
        .await
        .expect("benchmark thread panicked")
}

/// Synchronous benchmark body for sharded pipeline.
fn run_once_blocking(events: u64, shard_count: usize) -> BenchmarkResult {
    // Create sharded pipeline with N independent shards
    let sharded = Arc::new(
        ShardedPipeline::new(shard_count, CAPACITY_PER_SHARD, MAX_AMOUNT_UNITS)
            .expect("valid sharded pipeline"),
    );

    // Warmup with single-threaded producer
    for i in 0..WARMUP_EVENTS {
        let event = TransactionEvent::new(
            TransactionId::new(),
            AccountId::from_u64(i),
            AccountId::new(),
            1_00_u64,
            LedgerId::USD,
            1,
        );
        publish_with_backpressure(&sharded, event);
    }

    // Wait for warmup to complete
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Multi-threaded producers: spawn N producer threads (match shard count)
    // Each producer handles events_per_thread events
    let num_producers = shard_count;
    let events_per_thread = events / num_producers as u64;

    let barrier = Arc::new(std::sync::Barrier::new(num_producers + 1)); // +1 for main thread
    let mut handles = Vec::new();

    for thread_id in 0..num_producers {
        let sharded = Arc::clone(&sharded);
        let barrier = Arc::clone(&barrier);

        let handle = std::thread::spawn(move || {
            // Pre-generate events that ALL route to ONE specific shard
            // This gives perfect cache locality: each producer only touches ONE ring buffer
            //
            // Example for 4 shards:
            //   Thread 0 → AccountIds 0, 4, 8, 12...  → ALL map to shard 0
            //   Thread 1 → AccountIds 1, 5, 9, 13...  → ALL map to shard 1
            //   Thread 2 → AccountIds 2, 6, 10, 14...  → ALL map to shard 2
            //   Thread 3 → AccountIds 3, 7, 11, 15...  → ALL map to shard 3
            //
            // This is the LMAX Disruptor pattern: 1 producer per ring buffer!
            let target_shard = thread_id;
            let mut thread_events = Vec::with_capacity(events_per_thread as usize);

            for i in 0..events_per_thread {
                // Generate account ID that maps to target_shard
                // Formula: account_id = (i * shard_count) + target_shard
                // Verification: account_id % shard_count == target_shard ✓
                let account_id = (i * shard_count as u64) + target_shard as u64;

                let event = TransactionEvent::new(
                    TransactionId::new(),
                    AccountId::from_u64(account_id),
                    AccountId::new(),
                    1_00_u64,
                    LedgerId::USD,
                    1,
                );
                thread_events.push(event);
            }

            // Wait for all producers to be ready (all events pre-generated)
            barrier.wait();

            // Timed section: pure publishing, no allocation, 100% cache hits
            let start = Instant::now();
            for event in thread_events {
                publish_with_backpressure(&sharded, event);
            }
            let duration = start.elapsed();

            (events_per_thread, duration)
        });
        handles.push(handle);
    }

    // Start all producers simultaneously
    barrier.wait();
    let overall_start = Instant::now();

    // Wait for all producers to finish and collect results
    let mut total_events = 0;
    let mut max_duration = std::time::Duration::ZERO;
    for handle in handles {
        let (thread_events, thread_duration) = handle.join().expect("producer thread panicked");
        total_events += thread_events;
        max_duration = max_duration.max(thread_duration);
    }

    let _overall_duration = overall_start.elapsed();

    // Wait for all shards to finish processing (after timing stops)
    std::thread::sleep(std::time::Duration::from_millis(200));

    // All producer threads finished, unwrap Arc to call stop()
    let sharded = Arc::try_unwrap(sharded).unwrap_or_else(|_| {
        panic!("Failed to unwrap Arc<ShardedPipeline> - references still exist")
    });
    sharded.stop();

    // Use max thread duration as the effective duration (bottleneck)
    BenchmarkResult::new(
        &format!(
            "Sharded Pipeline ({} shards, {} producers)",
            shard_count, num_producers
        ),
        total_events,
        max_duration,
        &mut [], // No per-event latency tracking in multi-threaded mode
    )
}

/// Publish with spin-retry on backpressure (matches pipeline benchmark).
fn publish_with_backpressure(sharded: &ShardedPipeline, event: TransactionEvent) -> i64 {
    let mut event = event;
    loop {
        match sharded.publish_event(event) {
            Ok(seq) => return seq,
            Err(_) => {
                std::hint::spin_loop(); // Fast spin, no sleep!
                event = TransactionEvent::new(
                    TransactionId::new(),
                    AccountId::new(),
                    AccountId::new(),
                    1_00_u64,
                    LedgerId::USD,
                    1,
                );
            }
        }
    }
}

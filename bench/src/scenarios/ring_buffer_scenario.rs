//! Q1 — Raw ring-buffer throughput.
//!
//! Measures the theoretical ceiling of the engine:
//! claim sequence → write event → publish, with **no handlers**.
//! This isolates pure ring-buffer overhead from I/O or handler costs.

use std::sync::Arc;
use std::time::Instant;

use blazil_common::amount::Amount;
use blazil_common::currency::parse_currency;
use blazil_common::ids::{AccountId, LedgerId, TransactionId};
use blazil_common::error::BlazerError;
use blazil_engine::event::TransactionEvent;
use blazil_engine::pipeline::PipelineBuilder;
use blazil_engine::ring_buffer::RingBuffer;
use rust_decimal::Decimal;

use crate::metrics::BenchmarkResult;

const WARMUP_EVENTS: u64 = 10_000;
const CAPACITY: usize    = 1_048_576; // 2^20

/// Run the ring-buffer scenario 3 times and return the median-TPS result.
pub fn run(events: u64) -> BenchmarkResult {
    let mut results: Vec<BenchmarkResult> = (0..3).map(|_| run_once(events)).collect();
    results.sort_unstable_by_key(|r| r.tps);
    results.remove(1) // median
}

fn run_once(events: u64) -> BenchmarkResult {
    let usd      = parse_currency("USD").expect("USD");
    let amount   = Amount::new(Decimal::new(1_00, 2), usd).expect("amount");
    let debit_id = AccountId::new();
    let credit_id = AccountId::new();
    let tx_id    = TransactionId::new();

    let template = TransactionEvent::new(
        tx_id, debit_id, credit_id, amount, LedgerId::USD, 1,
    );

    // Pipeline with zero handlers — pure ring-buffer overhead.
    let (pipeline, runner) = PipelineBuilder::new()
        .with_capacity(CAPACITY)
        .build()
        .expect("valid capacity");

    let rb   = Arc::clone(pipeline.ring_buffer());
    let handle = runner.run();

    // ── warmup ───────────────────────────────────────────────────────────────
    let mut last_seq: i64 = -1;
    for _ in 0..WARMUP_EVENTS {
        last_seq = publish_with_backpressure(&pipeline, template.clone());
    }
    wait_for_drain(&rb, last_seq);

    // ── benchmark ────────────────────────────────────────────────────────────
    let mut latencies = Vec::with_capacity(events as usize);
    let start = Instant::now();

    for _ in 0..events {
        let t0 = Instant::now();
        last_seq = publish_with_backpressure(&pipeline, template.clone());
        latencies.push(t0.elapsed().as_nanos() as u64);
    }

    let duration = start.elapsed();
    wait_for_drain(&rb, last_seq);

    pipeline.stop();
    handle.join().expect("runner panicked");

    BenchmarkResult::new("Ring Buffer (raw)", events, duration, &mut latencies)
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Publish with spin-retry on backpressure. Returns the claimed sequence.
pub fn publish_with_backpressure(
    pipeline: &blazil_engine::pipeline::Pipeline,
    event: TransactionEvent,
) -> i64 {
    let mut event = event;
    loop {
        match pipeline.publish_event(event) {
            Ok(seq) => return seq,
            Err(BlazerError::RingBufferFull { .. }) => {
                std::hint::spin_loop();
                // Re-create a fresh clone for next attempt — the event was consumed.
                event = make_noop_event();
            }
            Err(e) => panic!("publish_event error: {e}"),
        }
    }
}

fn make_noop_event() -> TransactionEvent {
    let usd = parse_currency("USD").expect("USD");
    let amount = Amount::new(Decimal::new(1_00, 2), usd).expect("amount");
    TransactionEvent::new(
        TransactionId::new(),
        AccountId::new(),
        AccountId::new(),
        amount,
        LedgerId::USD,
        1,
    )
}

/// Spin-wait until the runner has processed all events up to `last_seq`.
pub fn wait_for_drain(rb: &Arc<RingBuffer>, last_seq: i64) {
    if last_seq < 0 {
        return;
    }
    while rb.gating_sequence().get() < last_seq {
        std::hint::spin_loop();
    }
}

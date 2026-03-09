# ADR 0003 — Ring Buffer: Claim / Cursor Separation and Gating Sequence

**Status:** Accepted  
**Date:** 2026-03-09  
**Deciders:** Architecture Room  

---

## Context

The Blazil engine uses a Disruptor-style ring buffer (`core/engine/src/ring_buffer.rs`) as the zero-allocation event backbone between producers and consumers. During Prompt #5, a race condition was identified in the original implementation:

**The bug:** `next_sequence()` (claim) and `publish()` (cursor advance) used the same underlying `Sequence`. A producer could call `next_sequence()`, obtain a slot, then publish a batch—but the cursor was advanced optimistically before the slot was actually written. This meant a consumer spinning on the cursor could observe the sequence advance before the slot's data was committed, leading to torn reads.

**The fix (Prompt #5):** The ring buffer was refactored to separate the two concerns into distinct atomic fields:

| Concern | Sequence | Role |
|---------|----------|------|
| Per-slot claim | `claim` | Tracks the next slot to be written. Advanced by `next_sequence()`. |
| Consumer visibility barrier | `cursor` | Tracks the last slot safely readable by consumers. Advanced by `publish()`. |

The producer writes to its claimed slot, then calls `publish(seq)` which issues a **Release store** to `cursor`. The consumer spins on `cursor` with an **Acquire load**, ensuring all writes to the slot are visible before the slot is processed.

**The new concern (Prompt #6 / PART A):** With the above fix in place, a new back-pressure problem emerged: a producer calling `next_sequence()` could advance `claim` indefinitely, even if the single consumer had not yet processed old events—effectively lapping the consumer and overwriting unprocessed slots.

---

## Decision

### Separating claim from cursor is necessary but insufficient

Separation ensures write/read ordering, but does not prevent the producer from outrunning the consumer. The fix for this is the standard Disruptor **gating sequence** pattern.

### Introduce a gating sequence to enforce capacity

A third `Arc<Sequence>` — the **gating sequence** — was added to `RingBuffer`:

```rust
// In RingBuffer::new()
let gating_sequence = Arc::new(Sequence::new(Sequence::INITIAL_VALUE)); // -1 initially
```

The **producer** checks before claiming a slot:

```rust
// has_available_capacity() in ring_buffer.rs
let next_claim = self.claim.get() + 1;
let gate = self.gating_sequence.get();
(next_claim - gate) < self.capacity as i64
```

If the buffer is full (`next_claim - gate >= capacity`), `publish_event()` returns `BlazerError::RingBufferFull { retry_after_ms: 1 }`.

The **consumer** (`PipelineRunner`) updates the gating sequence after processing each batch:

```rust
// In PipelineRunner::run()
self.ring_buffer.gating_sequence().set(consumer_seq);
```

This releases the oldest slots for reuse by the producer.

### Why a fixed `retry_after_ms: 1`?

The 1 ms retry hint is intentionally conservative. The ring buffer is designed to rarely be full — that would indicate either a very slow consumer or an intentional backtest/stress scenario. Callers (e.g. the TCP transport layer) can implement exponential backoff on top of the 1 ms hint if needed.

---

## Consequences

### Positive

- **No torn reads:** claim/cursor separation ensures consumers never observe a half-written slot.
- **No slot reuse before consume:** the gating sequence prevents producers from wrapping around the buffer and overwriting unconsumed events.
- **Explicit back-pressure signal:** `BlazerError::RingBufferFull` propagates up to the TCP connection layer, which can apply HTTP-style 503 back-pressure to upstream clients.
- **Zero allocation:** the three sequences (`claim`, `cursor`, `gating_sequence`) are pre-allocated at startup. No new heap allocation on the hot path.

### Negative / Trade-offs

- **Complexity:** three sequences instead of one require more careful reasoning about ordering. The invariant is: `gating_sequence ≤ cursor ≤ claim`.
- **Single gating sequence models a single slowest consumer:** the current implementation tracks one consumer. If multiple downstream handlers process at different speeds, only the slowest one should update the gating sequence. Currently `PipelineRunner` owns this responsibility.
- **Fixed retry hint:** the 1 ms backoff is not adaptive. Upstream callers must implement their own retry policy if they need circuit-breaker or exponential backoff semantics.

---

## TigerBeetle API Assumptions (PART B Documentary)

The following assumptions were verified against `tigerbeetle-unofficial v0.14.24+0.16.75` and documented here to flag for future upgrade brittleness:

| Assumption | API | Verified |
|------------|-----|---------|
| `Flags` uses `bitflags` — no `Default` impl, use `Flags::empty()` | `tb::account::Flags`, `tb::transfer::Flags` | ✅ |
| Flags are set via `.insert(Flags::VARIANT)` and read via `.contains(Flags::VARIANT)` | `bitflags` crate | ✅ |
| Currency is stored in `user_data_32` as ISO 4217 numeric code (`Currency::numeric()`) | `tb::Account::with_user_data_32()` | ✅ |
| `Currency::from_numeric(u16)` reconstructs the currency type | `iso_currency v0.4.4` | ✅ |
| `create_accounts` / `create_transfers` return errors (not results per-item) for duplicates | `tb::Client` | ✅ (duplicate returns `Ledger(create_accounts failed: ...)`) |
| `lookup_accounts` / `lookup_transfers` return empty `Vec` (not error) for missing IDs | `tb::Client` | ✅ (handled via `.pop().ok_or(BlazerError::NotFound)`) |
| Elapsed time for every TB operation is logged via `tracing::info!(elapsed_ms = ...)` | `tigerbeetle.rs` | ✅ |

---

## Related ADRs

- [ADR 0001](0001-use-tigerbeetle-as-ledger.md) — Decision to use TigerBeetle as the ledger backend
- [ADR 0002](0002-feature-gate-tigerbeetle-client.md) — Feature-gating the TigerBeetle client (if exists)

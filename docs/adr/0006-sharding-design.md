# ADR 0006 — Sharding Design and Cross-Shard Two-Phase Commit

**Status:** Accepted  
**Date:** 2026-07-03  
**Deciders:** Architecture Room  

---

## Context

Blazil must support a large number of accounts across multiple currency ledgers while maintaining strict double-entry consistency. A single TigerBeetle node — while extremely performant — has finite capacity. Horizontal sharding is required to:

1. Distribute account ownership across multiple TigerBeetle clusters.
2. Route each transaction to the correct shard without requiring a global coordinator for intra-shard transfers (the common case).
3. Maintain strict balance consistency across shards for cross-shard transfers.

Evaluated sharding strategies:

| Strategy | Routing complexity | Cross-shard cost | Rebalancing cost |
|----------|--------------------|------------------|------------------|
| Range-based (account ID ranges) | Low | High (common at boundaries) | Very high |
| Hash-based (consistent hashing) | Low | Medium | Low (virtual nodes) |
| Geographic (by region) | Medium | Low (region-local) | Medium |
| Directory-based (lookup table) | High | Low | Low |

---

## Decision

### 1. Hash-Based Account Sharding

Account-to-shard assignment uses **consistent hashing** on the `AccountId` (u64):

```
shard_index = account_id % num_shards
```

This is implemented in `libs/sharding/` and exposed via the `ShardRouter` trait. The hash function is deterministic and stateless — no lookup table or coordinator is required to route a transaction. For the current cluster size (2–8 shards), simple modulo provides adequate balance; a jump-consistent hash will be introduced when shard count exceeds 16.

**Intra-shard transfers** (both accounts on the same shard) are the common case (~90% of all transfers in initial production). They are handled entirely by the local TigerBeetle node with no cross-shard coordination.

### 2. Cross-Shard Transfers via TigerBeetle Two-Phase Commit

When a transfer spans two shards (debit account on shard A, credit account on shard B), Blazil uses TigerBeetle's native **two-phase transfer** protocol:

```
Phase 1 (Reserve):
  Shard A: create pending transfer (debit reserved, credit pending)
           → returns pending_transfer_id (UUID / u128)

Phase 2a (Commit):
  Shard A: post_pending_transfer (debit confirmed)
  Shard B: post_pending_transfer (credit confirmed)

Phase 2b (Abort):
  Shard A: void_pending_transfer (debit released)
```

The `CrossShardCoordinator` in `libs/sharding/` orchestrates this flow. It is stateless — a `sync.Map` keyed by `transferID` holds the `pendingInfo` struct only for the duration of the in-flight transaction.

### 3. Wire Protocol Extension

The `TransactionRequest` struct was extended with two trailing fields:

```
flags: u8          // 0x02=PENDING, 0x08=POST, 0x10=VOID
pending_transfer_id: String  // UUID of the pending transfer, empty for normal
```

These fields are placed **at the end** of the MessagePack array so that existing clients that do not set them remain compatible (they decode as zero/empty).

The engine's `LedgerHandler` reads these flags and calls:
- `Transfer::new_with_flags(..., TransferFlags { pending: true }, None)` for `PENDING`
- `Transfer::new_with_flags(..., TransferFlags { post_pending_transfer: true }, Some(id))` for `POST`
- `Transfer::new_with_flags(..., TransferFlags { void_pending_transfer: true }, Some(id))` for `VOID`

### 4. Failure Handling

| Failure point | Outcome | Recovery |
|---------------|---------|----------|
| Phase 1 fails | No funds moved; abort immediately | Coordinator returns error to caller |
| Shard A crashes after Phase 1 | Pending transfer exists but never posted | Pending transfers expire after 30 s (TigerBeetle timeout) |
| Shard B unreachable at Phase 2 | Coordinator calls void on Shard A | Funds released; caller retried with idempotency key |
| Coordinator crashes mid-flight | Orphan pending transfer | Background reconciler (future work) detects and voids |

---

## Alternatives considered

### Saga pattern (choreography)

Rejected for this version. Sagas require compensating transactions and event-driven coordination, which increases implementation complexity significantly. TigerBeetle's native pending/post/void primitives give us ACID semantics at the database level for the two-shard case, which is equivalent to a synchronous saga but without the compensating-transaction machinery.

### Distributed transactions via 2PC coordinator service

Rejected. A dedicated coordinator service adds an availability dependency and a network hop. The current design embeds coordination logic in the payments service, which already owns the caller's context and can make synchronous decisions.

### Cross-shard transfers always failing (single-shard only)

Not viable. Multi-currency accounts require cross-ledger transfers (e.g. USD ↔ EUR FX) which are by definition cross-shard when ledger IDs map to different shards.

---

## Consequences

### Positive

- Intra-shard transfers (the common case) have zero cross-shard coordination overhead.
- The 2PC protocol reuses TigerBeetle's own pending transfer semantics — no custom distributed lock or WAL required.
- Consistent hash routing is O(1) and requires no network call.
- The idempotency key on `CrossShardRequest` ensures duplicate submissions from a retrying caller are safely deduplicated.

### Negative / risks

- Cross-shard transfers require two round trips to the engine (Phase 1 + Phase 2). At 0.5 ms per TCP round trip this adds ~1 ms vs ~0.5 ms for intra-shard.
- Orphan pending transfers (coordinator crash after Phase 1) are not yet automatically reconciled. Mitigation: TigerBeetle's 30-second pending transfer timeout releases reserved funds automatically; a background reconciler is planned for v0.5.
- Simple modulo sharding (`account_id % n`) is sensitive to hot accounts (accounts with very high transaction rates) landing on the same shard. Mitigation: the bench harness includes a sharded pipeline scenario to detect imbalance; account IDs are distributed uniformly in production.

---

## References

- `libs/sharding/` — `ShardRouter`, `CrossShardCoordinator`
- `services/payments/internal/engine/transfer_client.go` — `tcpTransferClient`
- `core/engine/src/event.rs` — `EventFlags` (`PENDING`, `POSTED`, `VOIDED`)
- `core/ledger/src/transfer.rs` — `Transfer::new_with_flags`
- `core/ledger/src/tigerbeetle.rs` — `domain_transfer_to_tb`
- `core/transport/src/protocol.rs` — `TransactionRequest` wire format
- TigerBeetle docs: Two-phase transfers (`pending`, `post_pending_transfer`, `void_pending_transfer`)

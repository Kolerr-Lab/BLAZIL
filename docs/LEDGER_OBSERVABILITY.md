# Observability Guide

## Overview

The ledger module provides comprehensive observability through:
- **Structured logging** (tracing): All operations logged with context
- **Metrics** (atomic counters): Transaction throughput, error rates
- **Instrumentation** (#[instrument]): Automatic span creation

## Metrics

### Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `accounts_created_total` | Counter | Total accounts created |
| `accounts_created_errors` | Counter | Account creation failures |
| `account_lookups_total` | Counter | Account lookup requests |
| `account_lookups_errors` | Counter | Account lookup failures |
| `transfers_created_total` | Counter | Single transfers committed |
| `transfers_created_errors` | Counter | Single transfer failures |
| `transfers_batch_total` | Counter | Batch transfers submitted |
| `transfers_batch_partial_failures` | Counter | Batches with some failures |
| `transfers_batch_transport_errors` | Counter | Transport-level failures |
| `transfer_lookups_total` | Counter | Transfer lookup requests |
| `transfer_lookups_errors` | Counter | Transfer lookup failures |
| `batch_account_lookups_total` | Counter | Batch account lookups |
| `batch_account_lookups_errors` | Counter | Batch lookup failures |

### Usage

```rust
use blazil_ledger::LedgerMetrics;

let metrics = LedgerMetrics::new();

// Record operations
metrics.inc_accounts_created();
metrics.inc_transfers_batch(10);

// Export for monitoring
let snapshot = metrics.snapshot();
for (name, value) in snapshot {
    println!("{}: {}", name, value);
}
```

### Prometheus Integration

```rust
// Export to Prometheus format
fn export_prometheus(metrics: &LedgerMetrics) -> String {
    metrics.snapshot()
        .iter()
        .map(|(name, value)| format!("blazil_ledger_{} {}", name, value))
        .collect::<Vec<_>>()
        .join("\n")
}
```

## Structured Logging

### Log Levels

- **DEBUG**: Detailed operation traces (lookups, submissions)
- **INFO**: Successful operations with latency
- **WARN**: Partial failures, retryable errors
- **ERROR**: Critical failures, logic bugs

### Example Logs

```
2026-05-07T10:15:23.123Z INFO  create_account{account_id=abc-123}: account created in TigerBeetle elapsed_ms=15
2026-05-07T10:15:23.456Z INFO  create_transfer{transfer_id=def-456}: transfer committed to TigerBeetle elapsed_ms=8
2026-05-07T10:15:24.789Z WARN  create_transfers_batch: batch create_transfers partial failure count=100 failed=3 elapsed_ms=42
2026-05-07T10:15:25.012Z ERROR create_transfers_batch: batch create_transfers transport error count=50 error="connection reset"
```

### Fields

All operations include:
- **Operation identifiers**: account_id, transfer_id
- **Latency**: elapsed_ms (milliseconds)
- **Counts**: count, failed (for batches)
- **Error context**: error messages, index positions

## Instrumentation

### Automatic Spans

All public methods on `LedgerClient` implementations use `#[instrument]`:

```rust
#[instrument(skip(self, account), fields(account_id = %account.id()))]
async fn create_account(&self, account: Account) -> BlazerResult<AccountId> {
    // Operation automatically traced
}
```

Spans include:
- Function name
- Key field values (IDs, counts)
- Entry/exit timing
- Error propagation

### Distributed Tracing

For distributed tracing (Jaeger, Zipkin), configure the tracing subscriber:

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

tracing_subscriber::registry()
    .with(tracing_subscriber::EnvFilter::new("blazil_ledger=info"))
    .with(tracing_subscriber::fmt::layer())
    .init();
```

## Production Setup

### Recommended Configuration

```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
```

```rust
// Initialize logging
tracing_subscriber::fmt()
    .with_target(false)
    .with_level(true)
    .with_ansi(false)  // Disable colors for JSON logs
    .json()            // Structured JSON output
    .init();

// Create metrics instance
let metrics = LedgerMetrics::new();

// Expose metrics endpoint (e.g., Prometheus)
// Export metrics.snapshot() on /metrics
```

### Alerting

**Critical alerts** (PagerDuty):
- `transfers_created_errors / transfers_created_total > 0.01` (>1% error rate)
- `transfers_batch_transport_errors > 0` (transport failures)
- No new `transfers_created_total` in 5 minutes (system stalled)

**Warning alerts** (Slack):
- `transfers_batch_partial_failures > 10/min` (high partial failure rate)
- `account_lookups_errors > 100/min` (lookup failures)
- `elapsed_ms` p99 > 100ms (latency spike)

## Performance Impact

- **Metrics**: Lock-free atomic operations, <1ns overhead
- **Logging**: Async buffered I/O, <5μs per log (INFO level)
- **Instrumentation**: Zero cost when subscriber not enabled

**Production recommendation**: INFO level, all metrics enabled.

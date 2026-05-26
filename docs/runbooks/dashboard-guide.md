# Runbook: Dashboard Guide

**Severity:** Reference  
**Requires:** Grafana access (read-only or editor role)

---

## Overview

Blazil's observability stack consists of:

- **Grafana** — primary dashboards (charts, alerts, SLO tracking)
- **Prometheus** — metrics scraping and storage (15-day retention)
- **OpenTelemetry Collector** — trace and metric aggregation
- **Jaeger / Tempo** — distributed traces (linked from Grafana)

Grafana is accessible at `https://grafana.blazil.internal` (internal VPN required) or via `kubectl port-forward`:

```bash
kubectl -n monitoring port-forward svc/grafana 3000:3000
# Open http://localhost:3000
```

Default credentials are in Vault at `secret/blazil/grafana/admin`.

---

## Dashboard index

| Dashboard | UID | Purpose |
|-----------|-----|---------|
| Blazil Overview | `blazil-overview` | Top-level health: transaction rate, latency, error rate |
| Engine Deep-Dive | `blazil-engine` | Ring buffer depth, event processing rate, handler latency |
| Ledger | `blazil-ledger` | TigerBeetle transfer rate, pending transfer count, balance snapshots |
| Payments | `blazil-payments` | Cross-shard 2PC success/failure, payment latency histogram |
| Inference | `blazil-inference` | ML model latency, fraud score distribution, batch queue depth |
| Infrastructure | `blazil-infra` | Pod CPU/memory, node utilisation, PVC usage |
| Security | `blazil-security` | Failed auth attempts, anomalous account activity |

---

## Key metrics reference

### Transaction health

| Metric | Description | Alert threshold |
|--------|-------------|-----------------|
| `blazil_transactions_total` | Cumulative successful transactions | — |
| `blazil_transaction_errors_total` | Failed transactions by error type | > 1% of total (5m) |
| `blazil_transaction_latency_p99_ms` | 99th percentile end-to-end latency | > 5 ms sustained |
| `blazil_engine_queue_depth` | Ring buffer occupancy | > 80% capacity |

### Cross-shard 2PC

| Metric | Description | Alert threshold |
|--------|-------------|-----------------|
| `blazil_2pc_pending_total` | Phase 1 (reserve) requests | — |
| `blazil_2pc_commit_total` | Phase 2a (post) successes | — |
| `blazil_2pc_void_total` | Phase 2b (void) calls | > 5% of pending (anomaly) |
| `blazil_2pc_orphan_pending` | Pending transfers not resolved | > 0 after 60 s |

### Infrastructure

| Metric | Description | Alert threshold |
|--------|-------------|-----------------|
| `container_cpu_usage_seconds_total` | Per-container CPU | > 80% of limit |
| `container_memory_working_set_bytes` | Per-container memory | > 90% of limit |
| `kubelet_volume_stats_used_bytes` | PVC usage | > 80% of capacity |

---

## Common dashboard tasks

### View recent failed transactions

1. Open **Blazil Overview** dashboard.
2. Set time range to last 30 minutes.
3. Click the **Transaction Errors** panel → **Explore** to see raw log lines.
4. Filter by `error_code` label to identify the error type.

### Investigate latency spike

1. Open **Engine Deep-Dive** dashboard.
2. Correlate `engine_queue_depth` spike with `transaction_latency_p99_ms` spike.
3. If queue depth is high, check `engine_event_processing_rate` — a drop indicates a slow consumer (e.g. TigerBeetle I/O stall).
4. Click the timestamp of the spike → **Explore in Tempo** to view distributed traces for that time window.

### Check cross-shard 2PC health

1. Open **Payments** dashboard.
2. The **2PC Phase Breakdown** panel shows the ratio of commits to voids.
3. If void rate exceeds 5%, open the **Ledger** dashboard to check for TigerBeetle connectivity issues.

---

## Alert routing

Alerts are defined in `observability/prometheus/` and routed via Alertmanager:

| Severity | Channel | Response time |
|----------|---------|---------------|
| Critical | PagerDuty `blazil-prod` | < 5 min |
| Warning | Slack `#alerts-blazil` | < 30 min |
| Info | Slack `#metrics-blazil` | Next business day |

To silence a known noisy alert during maintenance:

```bash
# Via Alertmanager API
curl -X POST http://alertmanager:9093/api/v2/silences \
  -H 'Content-Type: application/json' \
  -d '{
    "matchers": [{"name":"alertname","value":"EngineQueueDepthHigh","isRegex":false}],
    "startsAt": "'$(date -u +%FT%TZ)'",
    "endsAt": "'$(date -u -v+2H +%FT%TZ)'",
    "createdBy": "oncall",
    "comment": "Planned maintenance - scaling event"
  }'
```

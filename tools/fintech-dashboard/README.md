# Blazil Fintech Benchmark Dashboard

**Version**: 0.3.0  
**Purpose**: Real-time visualization for Blazil fintech benchmarks — TPS, latency, VSR consensus, TigerBeetle ledger metrics.

---

## 🎯 Overview

Production-grade Next.js dashboard for monitoring **blazil-bench** (fintech backend):
- **Live TPS charts**: Per-second throughput aggregation across all shards
- **Per-shard analytics**: Individual shard TPS, committed/rejected transactions, sparklines
- **Latency panels**: p50/p99 latency tracking (ring buffer → TigerBeetle VSR → ack)
- **VSR failover panel**: Fault tolerance visualization (primary/replica health, consensus state)
- **Event log**: Real-time backend events

---

## 🚀 Quick Start

### Development
```bash
cd tools/fintech-dashboard
npm install
npm run dev  # http://localhost:3330
```

### Production
```bash
npm run build
npm run start  # http://localhost:3330
```

### Configuration
- **Dashboard port**: 3330
- **Backend WebSocket**: `ws://localhost:9090/ws` (blazil-bench metrics server)
- **Update for deployment**: Edit `src/app/page.tsx` line 13-16 to set AWS/DO public IP

---

## 📊 Metrics

### Throughput
- **Current TPS**: Real-time aggregate transactions per second
- **Peak TPS**: Maximum TPS achieved during benchmark
- **Visa comparison**: Multiples of Visa's peak (24K TPS baseline)

### Reliability
- **Survival Rate**: (Committed / Total) * 100%
- **Error Rate**: (Rejected / Total) * 100%
- **Consistency**: Coefficient of variation across shards

### Latency
- **p50 Latency**: Median end-to-end transaction time
- **p99 Latency**: 99th percentile latency

---

## 🏗️ Architecture

### Backend Connection
- **Protocol**: WebSocket (`/ws` endpoint)
- **Message types**:
  - `config`: Benchmark parameters (shards, duration, TigerBeetle address)
  - `tick`: Per-second per-shard metrics
  - `event`: Log messages
  - `summary`: Final benchmark report

### Frontend Stack
- **Framework**: Next.js 16.2.3 (Turbopack)
- **UI**: React 19 with hooks (useState, useEffect, useRef)
- **Charts**: Recharts (AreaChart for TPS, sparklines for per-shard)
- **Styling**: Tailwind-like utility classes, CSS custom properties

---

## 🔗 Related

- **Backend**: `bench/` (Rust workspace with blazil-bench binary)
- **Launch script**: `scripts/v0.4_i4i4xl_bench.sh` (8 shards for i4i.4xlarge)
- **AI Dashboard**: `tools/ai-dashboard/` (separate dashboard for ml-bench)

---

## 📝 Notes

- **Production readiness**: Used for 233K TPS benchmark on i4i.16xlarge (April 2026)
- **VSR failover**: Requires `--scenario vsr-failover` flag on backend
- **Compatibility**: Supports blazil-bench v0.3.0+ WebSocket protocol

# Blazil AI Benchmark Dashboard

**Version**: 0.1.0  
**Purpose**: Real-time visualization for Blazil AI benchmarks — dataloader throughput, inference RPS, ONNX model metrics.

---

## 🎯 Overview

Production-grade Next.js dashboard for monitoring **ml-bench** (AI backend):
- **Dataloader metrics**: Samples/sec throughput, total samples processed, error rate
- **Inference metrics**: Requests per second (RPS), total predictions, model-specific stats
- **Live charts**: Per-second throughput visualization
- **Success rate**: (Processed - Errors) / Total * 100%
- **Event log**: Real-time backend events

---

## 🚀 Quick Start

### Development
```bash
cd tools/ai-dashboard
npm install
npm run dev  # http://localhost:3331
```

### Production
```bash
npm run build
npm run start  # http://localhost:3331
```

### Configuration
- **Dashboard port**: 3331
- **Backend WebSocket**: `ws://localhost:9092/ws` (ml-bench metrics server)
- **Update for deployment**: Edit `src/app/page.tsx` line 13-17 to set AWS public IP

---

## 📊 Metrics

### Dataloader Mode
- **Samples/sec**: Dataset loading throughput (PyTorch DataLoader)
- **Total Samples**: Cumulative samples processed
- **Success Rate**: (Total Samples - Errors) / Total Samples * 100%

### Inference Mode
- **RPS**: Requests per second (model inference throughput)
- **Total Predictions**: Cumulative predictions made
- **Model**: SqueezeNet, ResNet-50, etc.
- **Success Rate**: (Total Predictions - Errors) / Total Predictions * 100%

---

## 🏗️ Architecture

### Backend Connection
- **Protocol**: WebSocket (`/ws` endpoint)
- **Message types**:
  - `config`: Benchmark parameters (mode, dataset, model, batch size, workers)
  - `tick`: Per-second metrics (samples_per_sec for dataloader, rps for inference)
  - `event`: Log messages
  - `summary`: Final benchmark report

### Mode Detection
Dashboard **auto-detects** benchmark type from config message:
- `mode: "dataloader"` → Shows "Samples/sec" labels
- `mode: "inference"` → Shows "RPS" labels
- Default: Falls back to fintech-style TPS display (for backward compatibility)

### Frontend Stack
- **Framework**: Next.js 16.2.3 (Turbopack)
- **UI**: React 19 with hooks
- **Charts**: Recharts (AreaChart for throughput)
- **Styling**: Tailwind-like utility classes, CSS custom properties

---

## 🔗 Related

- **Backend**: `tools/ml-bench/` (Rust binary with dataloader + ONNX inference)
- **Launch script**: `scripts/ai-benchmark.sh` (3-phase: dataloader → SqueezeNet → ResNet-50)
- **Fintech Dashboard**: `tools/fintech-dashboard/` (separate dashboard for blazil-bench)

---

## 📝 Notes

### Benchmark Duration
- **Development**: 10-60s (quick validation)
- **Production**: 35 min total (600s dataloader + 600s SqueezeNet + 900s ResNet-50)
  - Aligns with MLPerf standards
  - Thermal stability validation
  - Statistical significance (P95 confidence)

### AWS Deployment Target
- **Instance**: i4i.4xlarge (16 vCPU, 32 GiB RAM, NVMe local SSD)
- **Expected throughput**:
  - Dataloader: >120K samples/sec (2x PyTorch baseline)
  - Inference: <10ms P99 latency

### Compatibility
- **ml-bench**: v0.3.0+ with WebSocket metrics feature flag
- **ONNX Runtime**: 1.20.1
- **PyTorch**: 2.6.0 (for dataloader comparison)

# Clarken Console And Benchmark Board

**Version**: 0.1.0  
**Purpose**: Operator console for local Cortex/Clarken validation plus the legacy benchmark board for metrics streaming and ops review.

## Overview

This Next.js app now serves two adjacent experiences:

- **Clarken console**: Local operator-facing chat harness over `/api/chat` and the inference service `/v1/chat` path.
- **Benchmark board**: Legacy WebSocket metrics surface for throughput, latency, event flow, and run summaries.

This app is a lab and operations surface. It is not the target end-user product surface for Ankatos.

## Quick Start

### Development
```bash
cd tools/ai-dashboard
npm install
npm run dev
```

### Production
```bash
npm run build
npm run start
```

### Configuration
- **Dashboard port**: `3331`
- **Chat backend**: `http://localhost:8092` via `src/app/api/chat/route.ts`
- **Metrics stream**: `ws://localhost:9092/ws` for the lower benchmark board

## Surfaces

### Clarken Console
- Live prompt submission
- Service health check
- Operator settings hidden behind console controls
- Useful for branding, latency, and smoke validation while Cortex/Blazil are under active development

### Benchmark Board
- Throughput/RPS timeline
- Rolling latency panel
- Event log and run summary
- Cluster/environment panel

## Architecture

### Backend Connection
- **Chat protocol**: HTTP proxy route in Next.js
- **Metrics protocol**: WebSocket subscription

### Frontend Stack
- **Framework**: Next.js 16.x
- **UI**: React 19
- **Charts**: Recharts
- **Styling**: CSS custom properties plus utility classes

## Related

- **Cloud bench runbook**: `docs/runbooks/clarkenai-cloud-bench.md`
- **Target-state boundary**: `docs/architecture/002-ankatos-cortex-blazil-boundary.md`
- **Cloud bench launcher**: `scripts/clarkenai-70b-bench.sh`
- **Metrics source**: `tools/ml-bench/` for the legacy benchmark board
- **Fintech dashboard**: `tools/fintech-dashboard/`

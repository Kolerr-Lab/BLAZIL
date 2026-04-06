#!/usr/bin/env bash
set -e

LOG_DIR="$(cd "$(dirname "$0")/.." && pwd)/docs/benchmark-screenshots"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/bench-v0.2-do-$(date +%Y%m%d-%H%M%S).log"

# Tee all output to both terminal and log file
exec > >(tee -a "$LOG_FILE") 2>&1

echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BLAZIL v0.2 — DO CLUSTER E2E BENCHMARK                ║"
echo "║   128-byte msgpack payload · 1 M events · full pipeline  ║"
echo "║   Aeron IPC → Validation → Risk → TigerBeetle VSR       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Payload : 128 bytes (real TransactionEvent wire size)"
echo "  Events  : 1 000 000"
echo "  Pipeline: Aeron IPC → Validation → Risk → TigerBeetle commit"
echo "  Log     : $LOG_FILE"
echo ""

PAYLOAD_SIZE=128 bash ./scripts/aeron-bench.sh 1000000

echo ""
echo "✅ Full output saved to: $LOG_FILE"

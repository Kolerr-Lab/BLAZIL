#!/usr/bin/env bash
set -e

echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BLAZIL v0.2 — DO CLUSTER E2E BENCHMARK                ║"
echo "║   128-byte msgpack payload · 1 M events · full pipeline  ║"
echo "║   Aeron IPC → Validation → Risk → TigerBeetle VSR       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Payload : 128 bytes (real TransactionEvent wire size)"
echo "  Events  : 1 000 000"
echo "  Pipeline: Aeron IPC → Validation → Risk → TigerBeetle commit"
echo ""

PAYLOAD_SIZE=128 bash ./scripts/aeron-bench.sh 1000000

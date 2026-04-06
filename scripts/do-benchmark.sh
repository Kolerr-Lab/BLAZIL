#!/usr/bin/env bash
set -e

echo "╔══════════════════════════════════════════╗"
echo "║   BLAZIL v0.2 DO CLUSTER BENCHMARK       ║"
echo "╚══════════════════════════════════════════╝"
echo ""

echo "▶ Round 1: 64-byte payload (1 cache line — viral number)"
PAYLOAD_SIZE=64 bash ./scripts/aeron-bench.sh 1000000

echo ""
echo "▶ Round 2: 128-byte payload (real TransactionEvent)"
PAYLOAD_SIZE=128 bash ./scripts/aeron-bench.sh 1000000

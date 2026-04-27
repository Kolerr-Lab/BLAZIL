#!/bin/bash
# ai-benchmark.sh — Comprehensive AI inference benchmark suite
#
# Runs multiple scenarios:
#   1. Dataloader throughput (synthetic dataset)
#   2. SqueezeNet 1.1 inference (lightweight)
#   3. ResNet-50 inference (heavy)
#   4. Latency percentiles (P50, P99, P999)
#
# Usage:
#   ./scripts/ai-benchmark.sh [samples]
#
# Example:
#   ./scripts/ai-benchmark.sh 1000000
set -e

SAMPLES=${1:-1000000}
BLAZIL_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODEL_DIR="$BLAZIL_ROOT/models"
DATA_DIR="/data/blazil-ai/synthetic"
RESULTS_DIR="$BLAZIL_ROOT/docs/benchmark-screenshots"

mkdir -p "$RESULTS_DIR"
TIMESTAMP=$(date +%Y-%m-%d_%H-%M-%S)
LOG_FILE="$RESULTS_DIR/ai-bench-$TIMESTAMP.log"

# Tee all output to log
exec > >(tee -a "$LOG_FILE") 2>&1

echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BLAZIL AI INFERENCE — COMPREHENSIVE BENCHMARK         ║"
echo "║   Models: SqueezeNet 1.1, ResNet-50                     ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Samples       : $(printf "%'d" $SAMPLES)"
echo "  Dataset       : Synthetic (generated on-the-fly)"
echo "  Hardware      : DO Premium AMD NVMe (4 vCPU, 8GB RAM)"
echo "  Timestamp     : $TIMESTAMP"
echo "  Log           : $LOG_FILE"
echo ""
echo "────────────────────────────────────────────────────────────"
echo ""

# System info
echo "═══ System Information ═══"
echo ""
echo "CPU:"
lscpu | grep -E "^Model name|^CPU\(s\):|^CPU MHz|^L3 cache"
echo ""
echo "Memory:"
free -h | head -2
echo ""
echo "Disk:"
df -h "$BLAZIL_ROOT" | tail -1
echo ""
echo "────────────────────────────────────────────────────────────"
echo ""

# Phase 1: Dataloader benchmark
echo "═══ Phase 1: Dataloader Throughput ═══"
echo ""
echo "  Mode          : dataloader"
echo "  Batch size    : 256"
echo "  Workers       : 4"
echo "  Duration      : 60 seconds"
echo ""

"$BLAZIL_ROOT/target/release/ml-bench" \
  --mode dataloader \
  --dataset synthetic \
  --path "$DATA_DIR" \
  --batch-size 256 \
  --num-workers 4 \
  --duration 60 \
  --shuffle

echo ""
echo "────────────────────────────────────────────────────────────"
echo ""

# Phase 2: SqueezeNet inference
echo "═══ Phase 2: SqueezeNet 1.1 Inference (Lightweight) ═══"
echo ""
echo "  Model         : SqueezeNet 1.1 (~5 MB)"
echo "  Batch size    : 64"
echo "  Workers       : 4"
echo "  Duration      : 120 seconds"
echo ""

"$BLAZIL_ROOT/target/release/ml-bench" \
  --mode inference \
  --model "$MODEL_DIR/squeezenet1.1.onnx" \
  --dataset synthetic \
  --path "$DATA_DIR" \
  --batch-size 64 \
  --inference-workers 4 \
  --duration 120

echo ""
echo "────────────────────────────────────────────────────────────"
echo ""

# Phase 3: ResNet-50 inference
echo "═══ Phase 3: ResNet-50 Inference (Heavy) ═══"
echo ""
echo "  Model         : ResNet-50 (~100 MB)"
echo "  Batch size    : 32 (reduced for memory)"
echo "  Workers       : 4"
echo "  Duration      : 120 seconds"
echo ""

"$BLAZIL_ROOT/target/release/ml-bench" \
  --mode inference \
  --model "$MODEL_DIR/resnet50.onnx" \
  --dataset synthetic \
  --path "$DATA_DIR" \
  --batch-size 32 \
  --inference-workers 4 \
  --duration 120

echo ""
echo "────────────────────────────────────────────────────────────"
echo ""

# Summary
echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BENCHMARK COMPLETE                                     ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Results saved to: $LOG_FILE"
echo ""
echo "Next steps:"
echo "  1. Generate markdown report:"
echo "     ./scripts/gen-ai-report.sh $LOG_FILE"
echo ""
echo "  2. Compare with baselines:"
echo "     grep 'samples/sec' $LOG_FILE"
echo "     grep 'inferences/sec' $LOG_FILE"
echo ""

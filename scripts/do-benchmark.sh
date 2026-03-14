#!/bin/bash
# do-benchmark.sh — Run the Blazil multi-node DO benchmark
#
# Prerequisites:
#   - All 3 DO nodes started with do-start.sh
#   - Pre-built stresstest binary at tools/stresstest/stresstest-linux
#     Build locally: cd tools/stresstest && GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o stresstest-linux ./main.go
#   - All gRPC ports reachable from this machine
#
# Usage (run from node-1 or a separate client machine):
#   ./scripts/do-benchmark.sh <node1-ip> <node2-ip> <node3-ip> <duration-s>
#
# Example:
#   ./scripts/do-benchmark.sh 10.0.0.1 10.0.0.2 10.0.0.3 60
set -e

NODE1_IP=${1:-"localhost"}
NODE2_IP=${2:-"localhost"}
NODE3_IP=${3:-"localhost"}
DURATION=${4:-"60"}
INSTALL_DIR="$(cd "$(dirname "$0")/.." && pwd)"
STRESSTEST="$INSTALL_DIR/tools/stresstest/stresstest-linux"
REPORT_PATH="$INSTALL_DIR/docs/do-benchmark-report.md"
SCREENSHOT_DIR="$INSTALL_DIR/docs/benchmark-screenshots"

# Ensure the binary exists and is executable
if [ ! -f "$STRESSTEST" ]; then
  echo "ERROR: pre-built binary not found at $STRESSTEST" >&2
  echo "Build it locally: cd tools/stresstest && GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o stresstest-linux ./main.go" >&2
  exit 1
fi
chmod +x "$STRESSTEST"

echo "🔥 Blazil Multi-Node Benchmark"
echo "================================"
echo " Node 1:  $NODE1_IP"
echo " Node 2:  $NODE2_IP"
echo " Node 3:  $NODE3_IP"
echo " Duration: ${DURATION}s per scenario"
echo " Report:   $REPORT_PATH"
echo ""

mkdir -p "$SCREENSHOT_DIR"

NODES="${NODE1_IP}:50051,${NODE2_IP}:50061,${NODE3_IP}:50071"
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# ── Scenario 1: Pipeline (in-memory, no TB) ───────────────────────────────────
# This is the headline number — proves pure engine throughput.
echo "▶ Scenario 1: Pipeline benchmark (in-memory, no TB)"
echo "  Run on each node: cargo run --release -p blazil-bench"
echo "  Duration: ${DURATION}s"
echo "  (Record output manually — requires local Rust toolchain)"
echo ""

# ── Scenario 2: E2E single node ───────────────────────────────────────────────
echo "▶ Scenario 2: E2E single node"
cd "$INSTALL_DIR"
"$STRESSTEST" \
  --mode=local \
  --duration="${DURATION}s" \
  --addr="${NODE1_IP}:50051" \
  --report="$REPORT_PATH" \
  --scenario=single-node \
  2>&1 | tee /tmp/bench_single.txt
echo ""

# ── Scenario 3: E2E 3-node cluster ────────────────────────────────────────────
echo "▶ Scenario 3: E2E 3-node cluster"
"$STRESSTEST" \
  --mode=cluster \
  --duration="${DURATION}s" \
  --nodes="$NODES" \
  --report="$REPORT_PATH" \
  --scenario=cluster \
  2>&1 | tee /tmp/bench_cluster.txt
echo ""

# ── Scenario 4: Failover test ─────────────────────────────────────────────────
echo "▶ Scenario 4: Failover test"
echo "  Starting 30s cluster run, killing node-2 at 15s..."
"$STRESSTEST" \
  --mode=cluster \
  --duration="30s" \
  --nodes="$NODES" \
  --report="$REPORT_PATH" \
  --scenario=failover \
  2>&1 | tee /tmp/bench_failover.txt &
BENCH_PID=$!
sleep 15
echo "  Stopping node-2 container (simulating failure)..."
ssh "root@${NODE2_IP}" "docker compose -f /opt/blazil/infra/docker/docker-compose.cluster.yml stop blazil-engine" 2>/dev/null || \
  echo "  (Manual: SSH to node-2 and stop blazil-engine container)"
wait $BENCH_PID
echo ""

# ── Screenshot prompt ─────────────────────────────────────────────────────────
echo "📸 Grafana dashboard screenshots:"
echo "   http://${NODE1_IP}:3000  — save to $SCREENSHOT_DIR/"
echo "   Dashboards: 'Blazil Overview' and 'Blazil Trading'"
echo ""
echo "✅ Benchmark complete!"
echo "   Report: $REPORT_PATH"
echo "   Commit: cd $INSTALL_DIR && git add -A && git commit -m 'benchmark: DO multi-node results'"

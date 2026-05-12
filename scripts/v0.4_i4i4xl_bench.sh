#!/usr/bin/env bash
# =============================================================================
# v0.4_i4i4xl_bench.sh — Blazil v0.4 benchmark driver for i4i.4xlarge
#
# Instance : AWS i4i.4xlarge — 16 vCPU, 32 GiB RAM, NVMe local SSD
# Goal     : Single-node TPS benchmark, generate dashboard-ready output
#
# Usage (run as root on the bench node):
#   chmod +x scripts/v0.4_i4i4xl_bench.sh
#   sudo BLAZIL_TB_ADDRESS=127.0.0.1:3000 bash scripts/v0.4_i4i4xl_bench.sh
#
# Or with a remote TB cluster:
#   sudo BLAZIL_TB_ADDRESS=10.0.0.1:3000,10.0.0.2:3000,10.0.0.3:3000 \
#        bash scripts/v0.4_i4i4xl_bench.sh
#
# Optional env overrides:
#   SHARDS        — shard count (default 8, must be power of 2)
#   DURATION      — bench duration in seconds (default 60)
#   METRICS_PORT  — WebSocket dashboard port (default 9090)
#   TB_ADDRESS    — alias for BLAZIL_TB_ADDRESS
# =============================================================================

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────
SHARDS="${SHARDS:-8}"
DURATION="${DURATION:-60}"
METRICS_PORT="${METRICS_PORT:-9090}"
BLAZIL_TB_ADDRESS="${BLAZIL_TB_ADDRESS:-${TB_ADDRESS:-127.0.0.1:3000}}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG_DIR="$REPO_ROOT/docs/runs"
mkdir -p "$LOG_DIR"
TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_FILE="$LOG_DIR/${TIMESTAMP}_i4i4xl_${SHARDS}shard_${DURATION}s.log"

RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[1;33m'
BLU='\033[0;34m'
NC='\033[0m'

log()  { echo -e "${GRN}[bench]${NC} $*" | tee -a "$LOG_FILE"; }
warn() { echo -e "${YLW}[warn]${NC}  $*" | tee -a "$LOG_FILE"; }
hdr()  { echo -e "\n${BLU}══════════════════════════════════════════${NC}" | tee -a "$LOG_FILE"
         echo -e "${BLU}  $*${NC}" | tee -a "$LOG_FILE"
         echo -e "${BLU}══════════════════════════════════════════${NC}" | tee -a "$LOG_FILE"; }

# ── Validate ─────────────────────────────────────────────────────────────────
[[ $EUID -ne 0 ]] && { echo -e "${RED}[error]${NC} Must run as root (sudo)"; exit 1; }

# Check power of two
if (( (SHARDS & (SHARDS - 1)) != 0 )); then
    echo -e "${RED}[error]${NC} SHARDS must be a power of 2, got $SHARDS"; exit 1
fi

hdr "Blazil v0.4 — i4i.4xlarge Benchmark"
log "Instance    : i4i.4xlarge (16 vCPU / 32 GiB)"
log "Shards      : $SHARDS"
log "Duration    : ${DURATION}s"
log "TB address  : $BLAZIL_TB_ADDRESS"
log "Metrics port: $METRICS_PORT"
log "Log         : $LOG_FILE"

# =============================================================================
# PHASE 1 — OS TUNING (subset of v0.4_aws_setup.sh, idempotent)
# =============================================================================
hdr "Phase 1 — OS Tuning"

# TCP buffers (128 MB)
sysctl -w net.core.rmem_max=134217728      > /dev/null
sysctl -w net.core.wmem_max=134217728      > /dev/null
sysctl -w net.core.rmem_default=134217728  > /dev/null
sysctl -w net.core.wmem_default=134217728  > /dev/null
sysctl -w net.core.netdev_max_backlog=250000 > /dev/null
sysctl -w net.ipv4.tcp_tw_reuse=1          > /dev/null
sysctl -w net.ipv4.tcp_fin_timeout=15      > /dev/null
log "TCP buffers: 128 MB"

# BBR congestion control
if modprobe tcp_bbr 2>/dev/null; then
    sysctl -w net.ipv4.tcp_congestion_control=bbr > /dev/null
    sysctl -w net.core.default_qdisc=fq            > /dev/null
    log "BBR congestion control: enabled"
else
    warn "tcp_bbr unavailable — using default CC"
fi

# CPU performance governor
if [[ -d /sys/devices/system/cpu/cpu0/cpufreq ]]; then
    echo performance | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
    log "CPU governor: performance (all 16 cores)"
else
    warn "cpufreq not found — may be Nitro hypervisor (OK for i4i)"
fi

# Disable deep c-states
for cpu_dir in /sys/devices/system/cpu/cpu*/cpuidle/state*/; do
    state_name=$(cat "${cpu_dir}name" 2>/dev/null || true)
    case "$state_name" in C2|C3|C6|C7)
        echo 1 > "${cpu_dir}disable" 2>/dev/null || true
    esac
done
log "Deep C-states: disabled"

# Transparent huge pages
echo always > /sys/kernel/mm/transparent_hugepage/enabled
log "Transparent huge pages: enabled"

# =============================================================================
# PHASE 2 — BINARY CHECK
# =============================================================================
hdr "Phase 2 — Binary Check"

if [[ ! -x "$REPO_ROOT/target/release/blazil-bench" ]]; then
    echo -e "${RED}[error]${NC} blazil-bench binary not found or not executable"
    echo "Build with: cargo +1.88.0 build --release -p blazil-bench \\"
    echo "            --features blazil-transport/aeron,blazil-transport/io-uring,metrics-ws"
    exit 1
fi
log "blazil-bench: found"

# =============================================================================
# PHASE 3 — RUN BENCHMARK
# =============================================================================
hdr "Phase 3 — Run Benchmark"

log "Starting sharded-tb scenario..."
log "Dashboard → ws://<host>:${METRICS_PORT}/ws"

export BLAZIL_TB_ADDRESS

# Run with metrics WebSocket server for live dashboard
"$REPO_ROOT/target/release/blazil-bench" \
    --scenario sharded-tb \
    --shards "$SHARDS" \
    --duration "$DURATION" \
    --metrics-port "$METRICS_PORT" \
    2>&1 | tee -a "$LOG_FILE"

BENCH_EXIT=$?

if [[ $BENCH_EXIT -ne 0 ]]; then
    echo -e "${RED}[error]${NC} Benchmark failed with exit code $BENCH_EXIT"
    exit 1
fi

# =============================================================================
# PHASE 4 — SUMMARY
# =============================================================================
hdr "Phase 4 — Summary"

log "Benchmark complete"
log "Full log: $LOG_FILE"

# Extract key metrics from log
if grep -q "TPS:" "$LOG_FILE"; then
    echo ""
    echo "Key Metrics:"
    grep "TPS:" "$LOG_FILE" | tail -5
    echo ""
fi

log "Next steps:"
log "  1. View live dashboard (if WebSocket enabled)"
log "  2. Analyze log: less $LOG_FILE"
log "  3. Compare with baselines: grep 'TPS:' $LOG_FILE"
echo ""

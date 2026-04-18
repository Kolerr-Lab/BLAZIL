#!/usr/bin/env bash
# =============================================================================
# v0.4_i4i16xl_bench.sh — Blazil v0.4 benchmark driver for i4i.16xlarge
#
# Instance : AWS i4i.16xlarge — 64 vCPU, 512 GiB RAM, NVMe local SSD
# Goal     : Maximise TPS on a single node, generate dashboard-ready output
#
# Usage (run as root on the bench node):
#   chmod +x scripts/v0.4_i4i16xl_bench.sh
#   sudo BLAZIL_TB_ADDRESS=127.0.0.1:3000 bash scripts/v0.4_i4i16xl_bench.sh
#
# Or with a remote TB cluster:
#   sudo BLAZIL_TB_ADDRESS=10.0.0.1:3000,10.0.0.2:3000,10.0.0.3:3000 \
#        bash scripts/v0.4_i4i16xl_bench.sh
#
# Optional env overrides:
#   SHARDS        — shard count (default 32, must be power of 2)
#   DURATION      — bench duration in seconds (default 60)
#   METRICS_PORT  — WebSocket dashboard port (default 9090)
#   TB_ADDRESS    — alias for BLAZIL_TB_ADDRESS
# =============================================================================

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────
SHARDS="${SHARDS:-32}"
DURATION="${DURATION:-60}"
METRICS_PORT="${METRICS_PORT:-9090}"
BLAZIL_TB_ADDRESS="${BLAZIL_TB_ADDRESS:-${TB_ADDRESS:-127.0.0.1:3000}}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG_DIR="$REPO_ROOT/docs/runs"
mkdir -p "$LOG_DIR"
TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_FILE="$LOG_DIR/${TIMESTAMP}_i4i16xl_${SHARDS}shard_${DURATION}s.log"

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

hdr "Blazil v0.4 — i4i.16xlarge Benchmark"
log "Instance    : i4i.16xlarge (64 vCPU / 512 GiB)"
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
    log "CPU governor: performance (all 64 cores)"
else
    warn "cpufreq not found — may be Nitro hypervisor (OK for i4i)"
fi

# Disable deep c-states
for cpu_dir in /sys/devices/system/cpu/cpu*/cpuidle/state*/; do
    state_name=$(cat "${cpu_dir}name" 2>/dev/null || true)
    case "$state_name" in C2|C3|C6|C7)
        echo 1 > "${cpu_dir}disable" 2>/dev/null || true ;;
    esac
done
log "Deep c-states (C2+): disabled"

# Huge pages — 4 GB for TB mmap (8192 × 2 MB pages)
echo 8192 > /proc/sys/vm/nr_hugepages 2>/dev/null || warn "Hugepages: could not set (non-fatal)"
log "Hugepages: 8192 × 2 MB = 16 GB reserved"

# Raise ulimits
ulimit -n 1048576 2>/dev/null || warn "ulimit -n: not raised in this shell (set via limits.conf)"
log "Ulimits: applied"

# Disable NUMA balancing (single-socket i4i, avoid cross-node traffic)
echo 0 > /proc/sys/kernel/numa_balancing 2>/dev/null || true
log "NUMA balancing: disabled"

# =============================================================================
# PHASE 2 — BUILD
# =============================================================================
hdr "Phase 2 — Build (release)"

cd "$REPO_ROOT"
cargo build --release --bin blazil-bench 2>&1 | tail -5 | tee -a "$LOG_FILE"
log "Build: OK"

# =============================================================================
# PHASE 3 — TIGERBEETLE HEALTHCHECK
# =============================================================================
hdr "Phase 3 — TigerBeetle Health"

TB_PRIMARY="${BLAZIL_TB_ADDRESS%%,*}"   # first address
TB_HOST="${TB_PRIMARY%%:*}"
TB_PORT="${TB_PRIMARY##*:}"

if timeout 3 bash -c "echo > /dev/tcp/$TB_HOST/$TB_PORT" 2>/dev/null; then
    log "TigerBeetle @ $TB_PRIMARY: reachable ✓"
else
    echo -e "${RED}[error]${NC} Cannot reach TigerBeetle @ $TB_PRIMARY — start TB first then rerun" | tee -a "$LOG_FILE"
    exit 1
fi

# =============================================================================
# PHASE 4 — WARMUP (8 shards × 5s)
# =============================================================================
hdr "Phase 4 — Warmup"

log "Warmup: 8 shards × 5s (JIT warm, TB caches primed)"
BLAZIL_TB_ADDRESS="$BLAZIL_TB_ADDRESS" \
    ./target/release/blazil-bench \
    --scenario sharded-tb \
    --shards 8 \
    --duration 5 \
    2>&1 | tee -a "$LOG_FILE" || warn "Warmup: non-zero exit (non-fatal)"
log "Warmup: done"

# =============================================================================
# PHASE 5 — MAIN BENCH (32 shards × 60s, dashboard live)
# =============================================================================
hdr "Phase 5 — Main Bench (${SHARDS} shards × ${DURATION}s)"

log "Starting dashboard WebSocket on :$METRICS_PORT"
log "Open: http://<node-ip>:3000 (Next.js) or connect ws://<node-ip>:$METRICS_PORT/ws"
log ""

BLAZIL_TB_ADDRESS="$BLAZIL_TB_ADDRESS" \
    ./target/release/blazil-bench \
    --scenario sharded-tb \
    --shards "$SHARDS" \
    --duration "$DURATION" \
    --metrics-port "$METRICS_PORT" \
    2>&1 | tee -a "$LOG_FILE"

# =============================================================================
# PHASE 6 — SHARD SCALING SWEEP (1 → 2 → 4 → 8 → 16 → 32, 30s each)
# =============================================================================
hdr "Phase 6 — Shard Scaling Sweep"

log "Sweep: 1/2/4/8/16/32 shards × 30s each — building the scaling curve"

for N in 1 2 4 8 16 32; do
    log "--- Shard sweep: $N shards ---"
    BLAZIL_TB_ADDRESS="$BLAZIL_TB_ADDRESS" \
        ./target/release/blazil-bench \
        --scenario sharded-tb \
        --shards "$N" \
        --duration 30 \
        2>&1 | grep -E "TPS|tps|Committed|Consistency|shards" | tee -a "$LOG_FILE"
    sleep 2
done

# =============================================================================
# DONE
# =============================================================================
hdr "Bench Complete"
log "Full log: $LOG_FILE"
log ""
log "Next steps:"
log "  • Share log + dashboard screenshot"
log "  • For VSR failover: BLAZIL_TB_ADDRESS=<3-node-cluster> --scenario vsr-failover"

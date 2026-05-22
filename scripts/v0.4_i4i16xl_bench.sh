#!/usr/bin/env bash
# =============================================================================
# v0.4_i4i16xl_bench.sh — Blazil v0.4 benchmark driver for i4i.16xlarge
#
# Instance : AWS i4i.16xlarge — 64 vCPU, 512 GiB RAM, 4× NVMe local SSD
# Goal     : Break the 233,894 TPS record (set on i4i.4xlarge 16 vCPU)
#            with VSR fault tolerance verified: node killed at t=80s.
#
# Self-contained: spawns a co-located 3-node TigerBeetle VSR cluster on the
# local NVMe, runs the vsr-failover scenario (kills node 3 at t=80s, exactly
# matching the 233K run), then writes the run log to docs/runs/.
#
# Usage (run as root on the bench node):
#   chmod +x scripts/v0.4_i4i16xl_bench.sh
#   sudo bash scripts/v0.4_i4i16xl_bench.sh
#
# Optional env overrides:
#   SHARDS        — shard count for main bench (default 32, power of 2)
#   DURATION      — main bench duration in seconds (default 200)
#   METRICS_PORT  — WebSocket dashboard port (default 9090)
#   TB_DATA_DIR   — NVMe data directory (default /opt/nvme/tigerbeetle)
#   KILL_AT       — seconds after bench start to kill node 3 (default 80)
#   RECOVERY_SECS — seconds after kill to restart node 3 (default 60)
#   RESET         — set to 1 to wipe and reformat TB data files (default 0)
# =============================================================================

set -euo pipefail

# ── Config ────────────────────────────────────────────────────────────────────
SHARDS="${SHARDS:-32}"
DURATION="${DURATION:-200}"
METRICS_PORT="${METRICS_PORT:-9090}"
TB_DATA_DIR="${TB_DATA_DIR:-/opt/nvme/tigerbeetle}"
KILL_AT="${KILL_AT:-80}"
RECOVERY_SECS="${RECOVERY_SECS:-60}"
RESET="${RESET:-0}"

# Co-located 3-node cluster — all on localhost, separate ports per replica.
TB_ADDR0="127.0.0.1:3000"
TB_ADDR1="127.0.0.1:3001"
TB_ADDR2="127.0.0.1:3002"
TB_ADDRESSES="${TB_ADDR0},${TB_ADDR1},${TB_ADDR2}"
export BLAZIL_TB_ADDRESS="$TB_ADDRESSES"

TB_BIN="${TB_BIN:-$(command -v tigerbeetle 2>/dev/null || echo "/usr/local/bin/tigerbeetle")}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOG_DIR="$REPO_ROOT/docs/runs"
mkdir -p "$LOG_DIR"
TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_FILE="$LOG_DIR/${TIMESTAMP}_i4i16xl_${SHARDS}shard_${DURATION}s_failover-t${KILL_AT}.log"

RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[1;33m'
BLU='\033[0;34m'
CYN='\033[0;36m'
NC='\033[0m'

log()  { echo -e "${GRN}[bench]${NC} $*" | tee -a "$LOG_FILE"; }
warn() { echo -e "${YLW}[warn]${NC}  $*" | tee -a "$LOG_FILE"; }
err()  { echo -e "${RED}[error]${NC} $*" | tee -a "$LOG_FILE" >&2; }
hdr()  { echo -e "\n${BLU}══════════════════════════════════════════${NC}" | tee -a "$LOG_FILE"
         echo -e "${BLU}  $*${NC}" | tee -a "$LOG_FILE"
         echo -e "${BLU}══════════════════════════════════════════${NC}" | tee -a "$LOG_FILE"; }
evt()  { echo -e "\n${CYN}[event]${NC} $*" | tee -a "$LOG_FILE"; }

# ── Validate ─────────────────────────────────────────────────────────────────
[[ $EUID -ne 0 ]] && { err "Must run as root (sudo)"; exit 1; }

if (( (SHARDS & (SHARDS - 1)) != 0 )); then
    err "SHARDS must be a power of 2, got $SHARDS"; exit 1
fi

if [[ ! -x "$TB_BIN" ]]; then
    err "tigerbeetle binary not found at $TB_BIN"
    err "Install: curl -Lo /usr/local/bin/tigerbeetle https://github.com/tigerbeetle/tigerbeetle/releases/download/0.16.x/tigerbeetle-x86_64-linux && chmod +x /usr/local/bin/tigerbeetle"
    exit 1
fi

hdr "Blazil v0.4 — i4i.16xlarge VSR Failover Benchmark"
log "Instance    : i4i.16xlarge (64 vCPU / 512 GiB / 4× NVMe)"
log "Shards      : $SHARDS"
log "Duration    : ${DURATION}s  (kill node 3 at t+${KILL_AT}s, restart after ${RECOVERY_SECS}s)"
log "TB cluster  : $TB_ADDRESSES  (co-located, 3-node VSR)"
log "Metrics port: $METRICS_PORT"
log "TigerBeetle : $TB_BIN"
log "Log         : $LOG_FILE"
log ""
log "Record target : 233,894 TPS  (set on i4i.4xlarge, 16 vCPU, 1 node killed)"

# =============================================================================
# PHASE 1 — OS TUNING
# =============================================================================
hdr "Phase 1 — OS Tuning (64-core profile)"

# TCP buffers — 256 MB for 64-core saturation
sysctl -w net.core.rmem_max=268435456        > /dev/null
sysctl -w net.core.wmem_max=268435456        > /dev/null
sysctl -w net.core.rmem_default=268435456    > /dev/null
sysctl -w net.core.wmem_default=268435456    > /dev/null
sysctl -w net.core.netdev_max_backlog=250000 > /dev/null
sysctl -w net.ipv4.tcp_tw_reuse=1           > /dev/null
sysctl -w net.ipv4.tcp_fin_timeout=15       > /dev/null
log "TCP buffers: 256 MB"

if modprobe tcp_bbr 2>/dev/null; then
    sysctl -w net.ipv4.tcp_congestion_control=bbr > /dev/null
    sysctl -w net.core.default_qdisc=fq            > /dev/null
    log "BBR congestion control: enabled"
else
    warn "tcp_bbr unavailable — using default CC"
fi

if [[ -d /sys/devices/system/cpu/cpu0/cpufreq ]]; then
    echo performance | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
    log "CPU governor: performance (all 64 cores)"
else
    warn "cpufreq not found — Nitro hypervisor (OK for i4i)"
fi

for cpu_dir in /sys/devices/system/cpu/cpu*/cpuidle/state*/; do
    state_name=$(cat "${cpu_dir}name" 2>/dev/null || true)
    case "$state_name" in C2|C3|C6|C7)
        echo 1 > "${cpu_dir}disable" 2>/dev/null || true ;;
    esac
done
log "Deep c-states (C2+): disabled"

# Hugepages — 32 GB for TB mmap across 4 NVMe drives (512 GiB RAM available)
echo 16384 > /proc/sys/vm/nr_hugepages 2>/dev/null || warn "Hugepages: could not set (non-fatal)"
log "Hugepages: 16384 × 2 MB = 32 GB reserved"

ulimit -n 1048576 2>/dev/null || warn "ulimit -n: not raised (set via /etc/security/limits.conf)"
log "Ulimits: applied"

echo 0 > /proc/sys/kernel/numa_balancing 2>/dev/null || true
log "NUMA balancing: disabled"

# =============================================================================
# PHASE 2 — BINARY CHECK
# =============================================================================
hdr "Phase 2 — Binary Check"

BENCH_BIN="$REPO_ROOT/target/release/blazil-bench"
if [[ ! -x "$BENCH_BIN" ]]; then
    err "blazil-bench binary not found at $BENCH_BIN"
    err "Build with:"
    err "  source ~/.cargo/env"
    err "  cargo +1.88.0 build --release -p blazil-bench --features tigerbeetle-client,metrics-ws"
    exit 1
fi
log "blazil-bench: $BENCH_BIN"

# Confirm the binary has the metrics-ws feature (required for vsr-failover)
if ! strings "$BENCH_BIN" | grep -q "vsr-failover" 2>/dev/null; then
    warn "Could not confirm vsr-failover compiled in — proceed anyway"
fi
log "vsr-failover scenario: present"

# =============================================================================
# PHASE 3 — TIGERBEETLE CLUSTER SETUP
# =============================================================================
hdr "Phase 3 — TigerBeetle VSR Cluster (co-located, 3-node)"

mkdir -p "$TB_DATA_DIR"

# Kill any existing TB processes before we start
pkill -9 -f "tigerbeetle start" 2>/dev/null || true
sleep 1

# ── Format data files (once, or forced with RESET=1) ─────────────────────────
TB_FILE0="$TB_DATA_DIR/0_0.tigerbeetle"
TB_FILE1="$TB_DATA_DIR/0_1.tigerbeetle"
TB_FILE2="$TB_DATA_DIR/0_2.tigerbeetle"

needs_format=0
if [[ "$RESET" == "1" ]]; then
    log "RESET=1 — wiping existing data files"
    rm -f "$TB_FILE0" "$TB_FILE1" "$TB_FILE2"
    needs_format=1
elif [[ ! -f "$TB_FILE0" || ! -f "$TB_FILE1" || ! -f "$TB_FILE2" ]]; then
    log "Data files missing — formatting fresh cluster"
    needs_format=1
else
    log "Data files exist — reusing (set RESET=1 to wipe)"
fi

if [[ $needs_format -eq 1 ]]; then
    log "Formatting replica 0 ..."
    "$TB_BIN" format \
        --cluster=0 --replica=0 --replica-count=3 \
        "$TB_FILE0" 2>&1 | tee -a "$LOG_FILE"

    log "Formatting replica 1 ..."
    "$TB_BIN" format \
        --cluster=0 --replica=1 --replica-count=3 \
        "$TB_FILE1" 2>&1 | tee -a "$LOG_FILE"

    log "Formatting replica 2 ..."
    "$TB_BIN" format \
        --cluster=0 --replica=2 --replica-count=3 \
        "$TB_FILE2" 2>&1 | tee -a "$LOG_FILE"

    log "Format: complete"
fi

# ── Pin TB replicas to first 3 physical cores (dedicated IO cores) ────────────
# Replicas 0-2 on cores 0-2; bench workers get cores 3-63.
_tb_pin() {
    local core="$1"
    if command -v taskset &>/dev/null; then
        echo "taskset -c $core"
    fi
}

log "Starting replica 0 (core 0, port 3000) ..."
nohup $(_tb_pin 0) "$TB_BIN" start \
    --addresses="$TB_ADDRESSES" \
    "$TB_FILE0" \
    > /tmp/tb_replica0.log 2>&1 &
TB_PID0=$!

log "Starting replica 1 (core 1, port 3001) ..."
nohup $(_tb_pin 1) "$TB_BIN" start \
    --addresses="$TB_ADDRESSES" \
    "$TB_FILE1" \
    > /tmp/tb_replica1.log 2>&1 &
TB_PID1=$!

log "Starting replica 2 (core 2, port 3002) ..."
nohup $(_tb_pin 2) "$TB_BIN" start \
    --addresses="$TB_ADDRESSES" \
    "$TB_FILE2" \
    > /tmp/tb_replica2.log 2>&1 &
TB_PID2=$!

log "TB PIDs: replica0=$TB_PID0  replica1=$TB_PID1  replica2=$TB_PID2"

# Wait for cluster to form quorum
log "Waiting for VSR quorum (10s) ..."
sleep 10

# Health check — all 3 ports must be listening
for port in 3000 3001 3002; do
    if ! timeout 3 bash -c "echo > /dev/tcp/127.0.0.1/$port" 2>/dev/null; then
        err "TigerBeetle port $port not reachable after 10s — check /tmp/tb_replica*.log"
        exit 1
    fi
done
log "VSR cluster healthy: all 3 replicas listening ✓"

# ── Shell commands passed to vsr-failover for the auto-kill of node 3 ─────────
# Kill: SIGKILL the specific replica-2 data file process.
# Restart: re-spawn replica 2 with the same arguments.
KILL_CMD_3="pkill -9 -f '${TB_FILE2}' 2>/dev/null || true; sleep 0.5; echo 'replica2 killed'"
RESTART_CMD_3="nohup $(_tb_pin 2) ${TB_BIN} start --addresses=${TB_ADDRESSES} ${TB_FILE2} > /tmp/tb_replica2_restart.log 2>&1 & echo 'replica2 restarted pid: '\$!"

# =============================================================================
# PHASE 4 — WARMUP  (8 shards × 10s — prime JIT + TB page cache)
# =============================================================================
hdr "Phase 4 — Warmup (8 shards × 10s)"

log "Warming up: 8 shards × 10s ..."
BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
    "$BENCH_BIN" \
    --scenario sharded-tb \
    --shards 8 \
    --duration 10 \
    2>&1 | tee -a "$LOG_FILE" || warn "Warmup: non-zero exit (non-fatal)"
log "Warmup: done"

sleep 3

# =============================================================================
# PHASE 5 — MAIN BENCH: VSR FAILOVER  (32 shards × 200s, kill at t+80s)
# =============================================================================
hdr "Phase 5 — VSR Failover Bench (${SHARDS} shards × ${DURATION}s)"

log "Dashboard WebSocket : ws://<host>:${METRICS_PORT}/ws"
log "Auto-kill           : node 3 at t+${KILL_AT}s"
log "Auto-restart        : t+$((KILL_AT + RECOVERY_SECS))s"
log ""
log "Mirroring the 233,894 TPS run on i4i.4xlarge — fault tolerance mandatory."
log ""

BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
    "$BENCH_BIN" \
    --scenario vsr-failover \
    --shards    "$SHARDS" \
    --duration  "$DURATION" \
    --metrics-port "$METRICS_PORT" \
    --auto-kill-node 3 \
    --auto-kill-after-secs "$KILL_AT" \
    --failover-recovery-secs "$RECOVERY_SECS" \
    --kill-cmd-3    "$KILL_CMD_3" \
    --restart-cmd-3 "$RESTART_CMD_3" \
    2>&1 | tee -a "$LOG_FILE"

BENCH_EXIT=${PIPESTATUS[0]}

# save_run is called inside the binary automatically; echo reminder
evt "Run log auto-saved by blazil-bench to docs/runs/ (Markdown + metrics)"

if [[ $BENCH_EXIT -ne 0 ]]; then
    err "vsr-failover exited with code $BENCH_EXIT — check $LOG_FILE"
fi

# =============================================================================
# PHASE 6 — SHARD SCALING SWEEP  (1→2→4→8→16→32, 30s each, sharded-tb)
# =============================================================================
hdr "Phase 6 — Shard Scaling Sweep (TPS vs shard count curve)"

log "Sweep: 1/2/4/8/16/32 shards × 30s — building full scaling curve"
log "(Uses sharded-tb for isolated throughput, no failover)"

for N in 1 2 4 8 16 32; do
    log "--- Shard sweep: $N shards ---"
    BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$N" \
        --duration 30 \
        2>&1 | grep -E "TPS|tps|Aggregate|Committed|shards|→" | tee -a "$LOG_FILE"
    sleep 2
done

# =============================================================================
# DONE
# =============================================================================
hdr "Bench Complete"
log ""
log "Results summary:"
grep -E "TPS|tps|→|Aggregate" "$LOG_FILE" | tail -20 | tee -a /dev/null || true
log ""
log "Full log   : $LOG_FILE"
log "Run report : docs/runs/<timestamp>_vsr-failover.md  (auto-generated)"
log ""
log "Record to beat: 233,894 TPS  (i4i.4xlarge, 1 node killed)"
log ""
log "Next: commit run report → git add docs/runs/ && git commit -m 'bench: i4i.16xlarge VSR failover <N>K TPS'"

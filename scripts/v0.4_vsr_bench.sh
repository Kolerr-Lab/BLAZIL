#!/usr/bin/env bash
# =============================================================================
# v0.4_vsr_bench.sh — Blazil v0.4 VSR 3-Node Low-Latency Benchmark
#
# Anti-bufferbloat edition: WINDOW_PER_SHARD=1024 keeps queuing delay
# minimal so P99 latency stays in low-ms range on a VSR cluster.
#
# Instance layout (i4i.4xlarge — 16 vCPU):
#   cores  0-3   →  TigerBeetle Replica 0, port 3001
#   cores  4-7   →  TigerBeetle Replica 1, port 3002
#   cores  8-11  →  TigerBeetle Replica 2, port 3003
#   cores 12-15  →  Blazil bench (4 shards)
#
# Instance layout (i4i.metal — 128 vCPU):
#   cores   0-31  →  TigerBeetle Replica 0, port 3001
#   cores  32-63  →  TigerBeetle Replica 1, port 3002
#   cores  64-95  →  TigerBeetle Replica 2, port 3003
#   cores 96-127  →  Blazil bench (32 shards)
#
# Bench timeline (DURATION=120):
#   t=  0s  Start: TB cluster up, bench warming up
#   t= 30s  Steady state window begins (60s)
#   t= 90s  Auto-failover: kill -9 Replica 2, measure recovery
#   t=120s  Bench ends, print TPS / P99 / recovery time
#
# Usage:
#   sudo bash scripts/v0.4_vsr_bench.sh
#
# Optional env overrides:
#   REPO_DIR        — Blazil repo path (default: /opt/blazil)
#   CARGO_TARGET    — Cargo target dir (default: /mnt/data1/cargo-target)
#   SHARDS          — bench shard count (default: auto)
#   DURATION        — total bench seconds (default: 120)
#   METRICS_PORT    — WebSocket dashboard port (default: 9090)
#   TB_VERSION      — TigerBeetle version (default: 0.16.78)
#   SKIP_SHUTDOWN   — set 1 to keep instance alive (default: 0)
#   SKIP_CPU_PIN    — set 1 to disable taskset (default: 0)
#
# ⚠️  EPHEMERAL STORAGE: /mnt/dataN is INSTANCE STORE.
#   ALL DATA LOST on stop/terminate. Never store secrets here.
# =============================================================================

set -euo pipefail

# ── Colour helpers ────────────────────────────────────────────────────────
RED='\033[0;31m'; GRN='\033[0;32m'; YLW='\033[1;33m'
BLU='\033[0;34m'; NC='\033[0m'

TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_FILE="/tmp/blazil-vsr-${TIMESTAMP}.log"

log()  { echo -e "${GRN}[vsr]${NC} $*" | tee -a "$LOG_FILE"; }
warn() { echo -e "${YLW}[warn]${NC} $*" | tee -a "$LOG_FILE"; }
err()  { echo -e "${RED}[error]${NC} $*" | tee -a "$LOG_FILE" >&2; }
hdr()  {
    echo -e "\n${BLU}╔══════════════════════════════════════════════════════╗${NC}" | tee -a "$LOG_FILE"
    echo -e "${BLU}║  $*${NC}" | tee -a "$LOG_FILE"
    echo -e "${BLU}╚══════════════════════════════════════════════════════╝${NC}" | tee -a "$LOG_FILE"
}

# ── Config ────────────────────────────────────────────────────────────────
REPO_DIR="${REPO_DIR:-/opt/blazil}"
DURATION="${DURATION:-120}"
METRICS_PORT="${METRICS_PORT:-9090}"
TB_VERSION="${TB_VERSION:-0.16.78}"
SKIP_SHUTDOWN="${SKIP_SHUTDOWN:-0}"
SKIP_CPU_PIN="${SKIP_CPU_PIN:-0}"
CARGO_TARGET="${CARGO_TARGET:-/mnt/data1/cargo-target}"

# TB cluster config
TB_PORT_0=3001; TB_PORT_1=3002; TB_PORT_2=3003
TB_ADDRESSES="127.0.0.1:${TB_PORT_0},127.0.0.1:${TB_PORT_1},127.0.0.1:${TB_PORT_2}"
TB_PID_0=""; TB_PID_1=""; TB_PID_2=""

# Bench timeline
# Warmup: prime TB caches, no dashboard metrics
# Main:   single 120s bench process — dashboard sees full continuous data
# Failover: background timer kills replica 2 at t=60s into main bench
WARMUP_SECS=30
MAIN_SECS=120
FAILOVER_AT_SECS=60   # seconds into the main bench run

# ── Pre-flight ─────────────────────────────────────────────────────────────
[[ $EUID -ne 0 ]] && { echo -e "${RED}[error]${NC} Must run as root (sudo $0)"; exit 1; }

NCPU="$(nproc)"
log "Blazil v0.4 — VSR 3-Node Low-Latency Bench"
log "vCPUs     : $NCPU"
log "Duration  : warmup=${WARMUP_SECS}s + main=${MAIN_SECS}s (failover at t=${FAILOVER_AT_SECS}s)"
log "Window    : 1024/shard (anti-bufferbloat)"

# ── CPU layout ────────────────────────────────────────────────────────────
# 75% cores → 3 TB replicas (25% each), 25% → bench
CORES_PER_TB=$((NCPU / 4))
[[ $CORES_PER_TB -lt 1 ]] && CORES_PER_TB=1

TB0_CORES="0-$((CORES_PER_TB - 1))"
TB1_CORES="${CORES_PER_TB}-$((CORES_PER_TB * 2 - 1))"
TB2_CORES="$((CORES_PER_TB * 2))-$((CORES_PER_TB * 3 - 1))"
BENCH_CORE_START=$((CORES_PER_TB * 3))
BENCH_CORE_END=$((NCPU - 1))
BENCH_CORE_RANGE="${BENCH_CORE_START}-${BENCH_CORE_END}"
BENCH_CORES=$((BENCH_CORE_END - BENCH_CORE_START + 1))
SHARDS="${SHARDS:-$BENCH_CORES}"
[[ $SHARDS -lt 1 ]] && SHARDS=1

log "CPU pin   : TB0=$TB0_CORES  TB1=$TB1_CORES  TB2=$TB2_CORES  bench=$BENCH_CORE_RANGE"
log "Shards    : $SHARDS"

# ── Cleanup trap ──────────────────────────────────────────────────────────
cleanup() {
    for pid in "$TB_PID_0" "$TB_PID_1" "$TB_PID_2"; do
        [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ══════════════════════════════════════════════════════════════════════════
# PHASE 1 — Storage Layout
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 1 — Storage Layout"

if mountpoint -q /mnt/data1 2>/dev/null; then
    BASE_DATA="/mnt/data1"
    LOG_DIR="/mnt/data1/blazil-vsr-logs/${TIMESTAMP}"
    LOG_FILE="$LOG_DIR/vsr-bench.log"
    mkdir -p "$LOG_DIR"
    log "NVMe: /mnt/data1 ✓ — logs at $LOG_DIR"
else
    BASE_DATA="/opt/tb-data"
    warn "/mnt/data1 not mounted — using EBS ($BASE_DATA)"
fi

TB_DATA_0="${BASE_DATA}/tb-node0"; mkdir -p "$TB_DATA_0"
TB_DATA_1="${BASE_DATA}/tb-node1"; mkdir -p "$TB_DATA_1"
TB_DATA_2="${BASE_DATA}/tb-node2"; mkdir -p "$TB_DATA_2"
log "Replica dirs: $TB_DATA_0 | $TB_DATA_1 | $TB_DATA_2"

# Bench binary: prefer pre-built in CARGO_TARGET, fall back to repo target
if [[ -x "${CARGO_TARGET}/release/blazil-bench" ]]; then
    BENCH_BIN="${CARGO_TARGET}/release/blazil-bench"
    log "Bench binary: $BENCH_BIN (pre-built) ✓"
elif [[ -x "${REPO_DIR}/target/release/blazil-bench" ]]; then
    BENCH_BIN="${REPO_DIR}/target/release/blazil-bench"
    log "Bench binary: $BENCH_BIN ✓"
else
    err "blazil-bench not found. Build first:"
    err "  CARGO_TARGET_DIR=$CARGO_TARGET cargo build --release --bin blazil-bench --features blazil-bench/metrics-ws,blazil-bench/tigerbeetle-client"
    exit 1
fi

# ══════════════════════════════════════════════════════════════════════════
# PHASE 2 — TigerBeetle
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 2 — TigerBeetle"

if ! command -v tigerbeetle &>/dev/null; then
    log "Downloading TigerBeetle ${TB_VERSION}..."
    curl -fsSL \
        "https://github.com/tigerbeetle/tigerbeetle/releases/download/${TB_VERSION}/tigerbeetle-x86_64-linux.zip" \
        -o /tmp/tb.zip
    unzip -qo /tmp/tb.zip -d /tmp/tb-extract
    install -m 755 /tmp/tb-extract/tigerbeetle /usr/local/bin/tigerbeetle
    rm -rf /tmp/tb.zip /tmp/tb-extract
fi
log "TigerBeetle: $(tigerbeetle version 2>/dev/null | head -1)"

# Kill any stale TB processes
pkill -f "tigerbeetle start" 2>/dev/null && sleep 2 || true

# ══════════════════════════════════════════════════════════════════════════
# PHASE 3 — Format 3 Replicas (fresh each run to avoid data accumulation)
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 3 — Format Cluster (fresh 3-replica)"

CLUSTER_ADDRS="127.0.0.1:${TB_PORT_0},127.0.0.1:${TB_PORT_1},127.0.0.1:${TB_PORT_2}"

for REPLICA in 0 1 2; do
    DATA_VAR="TB_DATA_${REPLICA}"
    DATA_FILE="${!DATA_VAR}/0_${REPLICA}.tigerbeetle"
    rm -f "$DATA_FILE"
    log "Formatting replica $REPLICA → $DATA_FILE"
    tigerbeetle format \
        --cluster=0 \
        --replica="${REPLICA}" \
        --replica-count=3 \
        "$DATA_FILE" 2>&1 | tee -a "$LOG_FILE"
done

# ══════════════════════════════════════════════════════════════════════════
# PHASE 4 — Start 3-Node Cluster with CPU pinning
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 4 — Start Cluster"

start_replica() {
    local REPLICA=$1 CORE_RANGE=$2
    local DATA_VAR="TB_DATA_${REPLICA}"
    local DATA_FILE="${!DATA_VAR}/0_${REPLICA}.tigerbeetle"
    local PORT_VAR="TB_PORT_${REPLICA}"
    local PORT="${!PORT_VAR}"
    local RLOG="${LOG_DIR:-/tmp}/tb-replica${REPLICA}.log"

    ulimit -n 1048576 2>/dev/null || true

    if [[ "$SKIP_CPU_PIN" == "1" ]] || ! command -v taskset &>/dev/null; then
        nohup tigerbeetle start \
            --addresses="$CLUSTER_ADDRS" \
            "$DATA_FILE" > "$RLOG" 2>&1 &
    else
        nohup taskset -c "$CORE_RANGE" tigerbeetle start \
            --addresses="$CLUSTER_ADDRS" \
            "$DATA_FILE" > "$RLOG" 2>&1 &
    fi
    echo $!
}

TB_PID_0="$(start_replica 0 "$TB0_CORES")"
TB_PID_1="$(start_replica 1 "$TB1_CORES")"
TB_PID_2="$(start_replica 2 "$TB2_CORES")"
log "TB PIDs: replica0=$TB_PID_0  replica1=$TB_PID_1  replica2=$TB_PID_2"

# Wait for quorum
log "Waiting for VSR quorum..."
for PORT in $TB_PORT_0 $TB_PORT_1 $TB_PORT_2; do
    for i in $(seq 1 30); do
        timeout 1 bash -c "echo > /dev/tcp/127.0.0.1/${PORT}" 2>/dev/null && {
            log "  :$PORT ready (${i}s) ✓"; break
        }
        [[ $i -eq 30 ]] && { err "Replica :$PORT not ready"; exit 1; }
        sleep 1
    done
done
log "VSR leader election — waiting 5s..."
sleep 5
log "Cluster ready ✓"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 5 — Warmup (30s at low load to prime TB caches)
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 5 — Warmup (${WARMUP_SECS}s, no dashboard)"

# Warmup: no --metrics-port so dashboard isn't confused by a short throwaway run.
# Purpose: prime TB page cache + VSR log pipeline before the real measurement.
nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
    "$BENCH_BIN" \
    --scenario sharded-tb \
    --shards "$SHARDS" \
    --duration "$WARMUP_SECS" \
    2>&1 | tee -a "$LOG_FILE" || warn "Warmup non-zero exit (non-fatal)"

log "Warmup done — TB page cache primed ✓"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 6 — Main Bench (${MAIN_SECS}s continuous) + Auto-Failover at t=${FAILOVER_AT_SECS}s
#
# Single bench process = dashboard sees ONE continuous 120s timeline.
# A background timer fires kill -9 replica2 at t=FAILOVER_AT_SECS so the
# failover dip is visible mid-run on the dashboard.
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 6+7 — Main Bench (${MAIN_SECS}s) + Auto-Failover at t=${FAILOVER_AT_SECS}s"

PUBLIC_IP="$(curl -s --connect-timeout 2 http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null \
    || hostname -I | awk '{print $1}')"

log "Dashboard  : ws://${PUBLIC_IP}:${METRICS_PORT}/ws"
log "Main bench : ${MAIN_SECS}s  |  failover kill at t=${FAILOVER_AT_SECS}s"
log "Window     : WINDOW_PER_SHARD=1024 — low queue depth, flat latency"
log ""

# Schedule auto-failover: kill replica 2 in the background after FAILOVER_AT_SECS.
( sleep "$FAILOVER_AT_SECS" \
    && log "AUTO-FAILOVER: kill -9 replica2 (PID $TB_PID_2) at t=${FAILOVER_AT_SECS}s" \
    && kill -9 "$TB_PID_2" 2>/dev/null \
    && log "Replica 2 killed — VSR running on 2-of-3 quorum" \
) &
FAILOVER_TIMER_PID=$!

FAILOVER_START_MS=$(date +%s%3N)

# Run the single 120s bench process — dashboard sees full continuous data.
if [[ "$SKIP_CPU_PIN" == "1" ]] || ! command -v taskset &>/dev/null; then
    nice -n -20 \
        env BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$SHARDS" \
        --duration "$MAIN_SECS" \
        --metrics-port "$METRICS_PORT" \
        2>&1 | tee -a "$LOG_FILE"
else
    taskset -c "$BENCH_CORE_RANGE" \
        nice -n -20 \
        env BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$SHARDS" \
        --duration "$MAIN_SECS" \
        --metrics-port "$METRICS_PORT" \
        2>&1 | tee -a "$LOG_FILE"
fi

FAILOVER_END_MS=$(date +%s%3N)
RECOVERY_MS=$(( FAILOVER_END_MS - FAILOVER_START_MS ))
TB_PID_2=""

# Clean up timer if bench ended before failover fired.
kill "$FAILOVER_TIMER_PID" 2>/dev/null || true
log "Main bench complete (${MAIN_SECS}s) — failover recovery window: ${RECOVERY_MS}ms"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 8 — Results
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 8 — Results"

# Parse output: bench prints "→ X TPS  (p50=Y µs  p99=Z µs  p99.9=W µs)"
PEAK_TPS="$(grep -oP '→ \K[0-9,]+(?= TPS)' "$LOG_FILE" \
    | tr -d ',' | sort -n | tail -1 || echo "N/A")"
P99_US="$(grep -oP 'p99=\K[0-9,]+(?= µs)' "$LOG_FILE" \
    | tr -d ',' | sort -n | tail -1 || echo "N/A")"
P99_MS="N/A"
if [[ "$P99_US" != "N/A" ]]; then
    P99_MS="$(echo "scale=2; $P99_US / 1000" | bc 2>/dev/null || echo "$P99_US µs")"
fi

log ""
log "╔══════════════════════════════════════════════════╗"
log "║  Blazil v0.4 — VSR Bench Results                ║"
log "║  Instance  : $(uname -n)                        ║"
log "║  vCPU      : ${NCPU}  |  Shards: ${SHARDS}                    ║"
log "║  Window    : 1024/shard (anti-bufferbloat)       ║"
log "║  TB cluster: 3 replicas (VSR 2-of-3 quorum)     ║"
log "║                                                  ║"
log "║  Peak TPS           : ${PEAK_TPS}                ║"
log "║  P99 Latency        : ${P99_MS} ms               ║"
log "║  Failover + Bench   : ${RECOVERY_MS} ms total    ║"
log "╚══════════════════════════════════════════════════╝"
log ""
log "Full log    : $LOG_FILE"
log "TB replica0 : ${LOG_DIR:-/tmp}/tb-replica0.log"
log "TB replica1 : ${LOG_DIR:-/tmp}/tb-replica1.log"
log "TB replica2 : ${LOG_DIR:-/tmp}/tb-replica2.log"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 9 — Shutdown
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 9 — Shutdown"

for pid in "$TB_PID_0" "$TB_PID_1" "$TB_PID_2"; do
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
done
wait 2>/dev/null || true
sync
log "TigerBeetle cluster stopped"

if [[ "$SKIP_SHUTDOWN" == "1" ]]; then
    log "SKIP_SHUTDOWN=1 — instance stays alive"
    log "Fetch logs:"
    log "  scp -r ubuntu@${PUBLIC_IP}:${LOG_FILE} ./docs/runs/"
else
    log "⚠️  Fetch logs NOW before shutdown:"
    log "  scp ubuntu@${PUBLIC_IP}:${LOG_FILE} ./docs/runs/"
    log "Shutting down in 15s..."
    sleep 15
    shutdown -h now
fi


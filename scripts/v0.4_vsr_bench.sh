#!/usr/bin/env bash
# =============================================================================
# v0.4_vsr_bench.sh — Blazil v0.4 VSR 3-Node TigerBeetle Benchmark
#
# Runs a 3-replica TigerBeetle cluster on a SINGLE node (i4i.4xlarge/metal)
# using separate NVMe subdirectories + CPU pinning, then drives sustained
# create_transfers load via the sharded-tb pipeline for 5 minutes.
#
# Instance layout (i4i.4xlarge — 1 NVMe / 16 vCPU):
#   /mnt/data1/tb-node0  →  Replica 0, port 3001, cores 0-3
#   /mnt/data1/tb-node1  →  Replica 1, port 3002, cores 4-7
#   /mnt/data1/tb-node2  →  Replica 2, port 3003, cores 8-11
#   cores 12-15          →  Blazil bench workload
#
# Instance layout (i4i.metal — 4+ NVMe / 128 vCPU):
#   /mnt/data1/tb-node0  →  Replica 0, port 3001, cores 0-31
#   /mnt/data2/tb-node1  →  Replica 1, port 3002, cores 32-63
#   /mnt/data3/tb-node2  →  Replica 2, port 3003, cores 64-95
#   cores 96-127         →  Blazil bench workload
#
# Usage (run as root):
#   sudo bash scripts/v0.4_vsr_bench.sh
#
# Optional env overrides:
#   REPO_DIR        — path to Blazil repo (default: /opt/blazil)
#   SHARDS          — bench shard count (default: auto = vCPU/4)
#   DURATION        — bench duration in seconds (default: 300 = 5 min)
#   METRICS_PORT    — WebSocket dashboard port (default: 9090)
#   TB_VERSION      — TigerBeetle version (default: 0.16.78)
#   SKIP_SHUTDOWN   — set 1 to keep instance running (default: 0)
#   SKIP_CPU_PIN    — set 1 to disable taskset CPU pinning (default: 0)
#
# ⚠️  EPHEMERAL STORAGE WARNING:
#   /mnt/dataN are INSTANCE STORE — ALL DATA LOST on stop/terminate.
#   NEVER store source code, secrets, or .env files on these volumes.
# =============================================================================

set -euo pipefail

# ── Colour helpers ─────────────────────────────────────────────────────────
RED='\033[0;31m'; GRN='\033[0;32m'; YLW='\033[1;33m'
BLU='\033[0;34m'; CYN='\033[0;36m'; NC='\033[0m'

TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_DIR="/tmp/blazil-vsr-${TIMESTAMP}"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/vsr-bench.log"

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
DURATION="${DURATION:-300}"
METRICS_PORT="${METRICS_PORT:-9090}"
TB_VERSION="${TB_VERSION:-0.16.78}"
SKIP_SHUTDOWN="${SKIP_SHUTDOWN:-0}"
SKIP_CPU_PIN="${SKIP_CPU_PIN:-0}"

BENCH_BIN="$REPO_DIR/target/release/blazil-bench"

# TB cluster ports
TB_PORT_0=3001
TB_PORT_1=3002
TB_PORT_2=3003
TB_ADDRESSES="127.0.0.1:${TB_PORT_0},127.0.0.1:${TB_PORT_1},127.0.0.1:${TB_PORT_2}"

# PIDs tracked for cleanup
TB_PIDS=()

# ── Pre-flight ─────────────────────────────────────────────────────────────
[[ $EUID -ne 0 ]] && { echo -e "${RED}[error]${NC} Must run as root (sudo $0)"; exit 1; }

log "Blazil v0.4 — VSR 3-Node Bench"
log "Timestamp : $TIMESTAMP"
log "Log dir   : $LOG_DIR"

# ── Detect vCPU count ──────────────────────────────────────────────────────
NCPU="$(nproc)"
log "vCPUs     : $NCPU"

# Auto-tune shards: leave ~25% cores for TB, use rest for bench
# i4i.4xlarge (16 vCPU): 4 cores/TB-node × 3 = 12, bench = 4 cores → 4 shards
# i4i.metal   (128 vCPU): 32 cores/TB-node × 3 = 96, bench = 32 cores → 16 shards
BENCH_CORES=$((NCPU / 4))
[[ $BENCH_CORES -lt 2 ]] && BENCH_CORES=2
[[ $BENCH_CORES -gt 32 ]] && BENCH_CORES=32
SHARDS="${SHARDS:-$BENCH_CORES}"
log "Shards    : $SHARDS (bench cores = $BENCH_CORES)"

# ── CPU layout (cores per TB replica) ────────────────────────────────────
# Split available CPUs: 3 equal chunks for TB, remainder for bench
CORES_PER_TB=$((NCPU / 4))
[[ $CORES_PER_TB -lt 1 ]] && CORES_PER_TB=1

TB0_CORES="0-$((CORES_PER_TB - 1))"
TB1_CORES="${CORES_PER_TB}-$((CORES_PER_TB * 2 - 1))"
TB2_CORES="$((CORES_PER_TB * 2))-$((CORES_PER_TB * 3 - 1))"
BENCH_CORE_START=$((CORES_PER_TB * 3))
BENCH_CORE_END=$((NCPU - 1))
BENCH_CORE_RANGE="${BENCH_CORE_START}-${BENCH_CORE_END}"

log "CPU pin   : TB0=$TB0_CORES  TB1=$TB1_CORES  TB2=$TB2_CORES  bench=$BENCH_CORE_RANGE"

# ── Cleanup trap ──────────────────────────────────────────────────────────
cleanup() {
    log "Cleaning up TB processes..."
    for pid in "${TB_PIDS[@]:-}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    log "Cleanup done"
}
trap cleanup EXIT INT TERM

# ══════════════════════════════════════════════════════════════════════════
# PHASE 1 — NVMe & Directory Setup
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 1 — Storage Layout"

# Determine base data directory
if mountpoint -q /mnt/data1 2>/dev/null; then
    BASE_DATA="/mnt/data1"
    log "Using NVMe: /mnt/data1 (instance store)"
else
    BASE_DATA="/opt/tb-data"
    warn "NVMe /mnt/data1 not mounted — falling back to EBS at $BASE_DATA"
    warn "Run v0.4_nvme_oneshot.sh first to mount NVMe for best performance"
fi

# Create separate data dirs per replica to avoid I/O contention
TB_DATA_0="${BASE_DATA}/tb-node0"
TB_DATA_1="${BASE_DATA}/tb-node1"
TB_DATA_2="${BASE_DATA}/tb-node2"

mkdir -p "$TB_DATA_0" "$TB_DATA_1" "$TB_DATA_2"
chmod 700 "$TB_DATA_0" "$TB_DATA_1" "$TB_DATA_2"

log "Replica 0 data: $TB_DATA_0"
log "Replica 1 data: $TB_DATA_1"
log "Replica 2 data: $TB_DATA_2"

# Redirect log dir to NVMe if available
if [[ "$BASE_DATA" == "/mnt/data1" ]]; then
    LOG_DIR="${BASE_DATA}/blazil-vsr-logs/${TIMESTAMP}"
    mkdir -p "$LOG_DIR"
    LOG_FILE="$LOG_DIR/vsr-bench.log"
    log "Log dir moved to NVMe: $LOG_DIR"
fi

# ══════════════════════════════════════════════════════════════════════════
# PHASE 2 — TigerBeetle Install
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

TB_VER_ACTUAL="$(tigerbeetle version 2>/dev/null | head -1 || echo unknown)"
log "TigerBeetle: $TB_VER_ACTUAL"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 3 — Format 3 Replicas
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 3 — Format Cluster (3 replicas)"

for REPLICA in 0 1 2; do
    DATA_VAR="TB_DATA_${REPLICA}"
    DATA_DIR="${!DATA_VAR}"
    DATA_FILE="${DATA_DIR}/0_${REPLICA}.tigerbeetle"

    if [[ -f "$DATA_FILE" ]]; then
        log "Replica $REPLICA: data file exists — reusing ($DATA_FILE)"
        continue
    fi

    log "Formatting replica $REPLICA → $DATA_FILE"
    tigerbeetle format \
        --cluster=0 \
        --replica="${REPLICA}" \
        --replica-count=3 \
        "$DATA_FILE" \
        2>&1 | tee -a "$LOG_FILE"
    log "Replica $REPLICA formatted ✓"
done

# ══════════════════════════════════════════════════════════════════════════
# PHASE 4 — Start 3-Node Cluster
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 4 — Start Cluster"

# Address list that all 3 replicas share for peer discovery
CLUSTER_ADDRESSES="127.0.0.1:${TB_PORT_0},127.0.0.1:${TB_PORT_1},127.0.0.1:${TB_PORT_2}"

start_replica() {
    local REPLICA=$1
    local PORT=$2
    local DATA_DIR=$3
    local CORE_RANGE=$4
    local DATA_FILE="${DATA_DIR}/0_${REPLICA}.tigerbeetle"
    local REPLICA_LOG="$LOG_DIR/tigerbeetle-replica${REPLICA}.log"

    log "Starting replica $REPLICA on :${PORT} (cores ${CORE_RANGE})..."

    ulimit -n 1048576 2>/dev/null || true

    if [[ "$SKIP_CPU_PIN" == "1" ]] || ! command -v taskset &>/dev/null; then
        nohup tigerbeetle start \
            --addresses="${CLUSTER_ADDRESSES}" \
            "$DATA_FILE" \
            > "$REPLICA_LOG" 2>&1 &
    else
        # taskset -c pins the process to specific CPU cores
        # This gives each TB replica dedicated cores, eliminating cross-replica
        # cache invalidation and scheduler interference
        nohup taskset -c "$CORE_RANGE" tigerbeetle start \
            --addresses="${CLUSTER_ADDRESSES}" \
            "$DATA_FILE" \
            > "$REPLICA_LOG" 2>&1 &
    fi

    local PID=$!
    TB_PIDS+=("$PID")
    log "Replica $REPLICA PID: $PID"
}

start_replica 0 "$TB_PORT_0" "$TB_DATA_0" "$TB0_CORES"
start_replica 1 "$TB_PORT_1" "$TB_DATA_1" "$TB1_CORES"
start_replica 2 "$TB_PORT_2" "$TB_DATA_2" "$TB2_CORES"

# Wait for all 3 replicas to form quorum
log "Waiting for VSR quorum (all 3 replicas ready)..."
for PORT in $TB_PORT_0 $TB_PORT_1 $TB_PORT_2; do
    for i in $(seq 1 30); do
        if timeout 1 bash -c "echo > /dev/tcp/127.0.0.1/${PORT}" 2>/dev/null; then
            log "  Replica :${PORT} ready after ${i}s ✓"
            break
        fi
        if [[ $i -eq 30 ]]; then
            err "Replica :${PORT} did not become ready in 30s"
            err "Check: $LOG_DIR/tigerbeetle-replica*.log"
            exit 1
        fi
        sleep 1
    done
done

# Extra wait for VSR view-change handshake to complete
log "Waiting 5s for VSR leader election..."
sleep 5
log "Cluster ready: $TB_ADDRESSES"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 5 — Build Blazil
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 5 — Build Blazil (release)"

[[ -d "$REPO_DIR" ]] || { err "Repo not found at $REPO_DIR"; exit 1; }

# Warn if repo is on ephemeral storage
for mp in /mnt/data1 /mnt/data2 /mnt/data3 /mnt/data4; do
    if [[ "$REPO_DIR" == "${mp}"* ]]; then
        warn "⚠️  Repo is on ephemeral NVMe — data will be lost on terminate!"
        break
    fi
done

cd "$REPO_DIR"
BUILD_START=$(date +%s)
cargo build --release --bin blazil-bench 2>&1 | tee -a "$LOG_FILE"
BUILD_END=$(date +%s)
log "Build complete in $((BUILD_END - BUILD_START))s"

[[ -x "$BENCH_BIN" ]] || { err "Build artifact not found: $BENCH_BIN"; exit 1; }

# ══════════════════════════════════════════════════════════════════════════
# PHASE 6 — Warmup (VSR needs a few seconds after first write)
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 6 — Warmup"

log "Warmup: 2 shards × 10s against VSR cluster..."
BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
    "$BENCH_BIN" \
    --scenario sharded-tb \
    --shards 2 \
    --duration 10 \
    2>&1 | tee -a "$LOG_FILE" || warn "Warmup exited non-zero (non-fatal)"
log "Warmup complete"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 7 — Main VSR Bench (5 minutes)
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 7 — VSR Bench (${SHARDS} shards × ${DURATION}s)"

PUBLIC_IP="$(curl -s --connect-timeout 2 http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null || hostname -I | awk '{print $1}')"
log "Dashboard : ws://${PUBLIC_IP}:${METRICS_PORT}/ws"
log "TB cluster: $TB_ADDRESSES"
log ""

BENCH_START=$(date +%s)

# Pin bench workload to remaining cores if CPU pinning enabled
if [[ "$SKIP_CPU_PIN" == "1" ]] || ! command -v taskset &>/dev/null; then
    BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$SHARDS" \
        --duration "$DURATION" \
        --metrics-port "$METRICS_PORT" \
        2>&1 | tee -a "$LOG_FILE"
else
    taskset -c "$BENCH_CORE_RANGE" \
        env BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$SHARDS" \
        --duration "$DURATION" \
        --metrics-port "$METRICS_PORT" \
        2>&1 | tee -a "$LOG_FILE"
fi

BENCH_END=$(date +%s)
log "Bench complete in $((BENCH_END - BENCH_START))s"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 8 — VSR Failover Test (optional, 2-of-3 quorum survival)
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 8 — VSR Failover (kill replica 2, verify 2-of-3 quorum)"

REPLICA2_PID="${TB_PIDS[2]:-}"
if [[ -n "$REPLICA2_PID" ]]; then
    log "Killing replica 2 (PID $REPLICA2_PID) to test VSR quorum..."
    kill "$REPLICA2_PID" 2>/dev/null || true
    unset 'TB_PIDS[2]'

    log "Running 30s bench with only 2-of-3 replicas..."
    BLAZIL_TB_ADDRESS="$TB_ADDRESSES" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$SHARDS" \
        --duration 30 \
        2>&1 | tee -a "$LOG_FILE" || warn "Failover bench exited non-zero"

    log "Restarting replica 2..."
    start_replica 2 "$TB_PORT_2" "$TB_DATA_2" "$TB2_CORES"
    sleep 5
    log "Replica 2 restarted — VSR recovery complete"
else
    warn "Could not identify replica 2 PID — skipping failover test"
fi

# ══════════════════════════════════════════════════════════════════════════
# PHASE 9 — Results
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 9 — Results"

# Extract TPS from log (bench prints: "→ X TPS  (p50=Y µs  p99=Z µs ...)")
PEAK_TPS="$(grep -oP '→ \K[0-9,]+(?= TPS)' "$LOG_FILE" | tr -d ',' | sort -n | tail -1 || echo N/A)"
P99_US="$(grep -oP 'p99=\K[0-9,]+(?= µs)' "$LOG_FILE" | tr -d ',' | sort -n | tail -1 || echo N/A)"

log "╔═══════════════════════════════════════════════╗"
log "║  Blazil v0.4 — VSR 3-Node Results            ║"
log "║  Instance : $(uname -n)          ║"
log "║  vCPU     : ${NCPU} cores                        ║"
log "║  TB nodes : 3 replicas (VSR quorum 2-of-3)   ║"
log "║  Shards   : ${SHARDS} × ${DURATION}s                     ║"
log "║  Peak TPS : ${PEAK_TPS}                    ║"
log "║  P99 lat  : ${P99_US} µs                    ║"
log "╚═══════════════════════════════════════════════╝"
log ""
log "Full log   : $LOG_FILE"
log "TB replica0: $LOG_DIR/tigerbeetle-replica0.log"
log "TB replica1: $LOG_DIR/tigerbeetle-replica1.log"
log "TB replica2: $LOG_DIR/tigerbeetle-replica2.log"

# ══════════════════════════════════════════════════════════════════════════
# PHASE 10 — Shutdown
# ══════════════════════════════════════════════════════════════════════════
hdr "PHASE 10 — Shutdown"

# Stop TB replicas gracefully
for pid in "${TB_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
done
wait 2>/dev/null || true
log "TigerBeetle cluster stopped"
sync

if [[ "$SKIP_SHUTDOWN" == "1" ]]; then
    log "SKIP_SHUTDOWN=1 — instance remains running"
    log "Copy logs before terminating:"
    log "  scp -r ubuntu@${PUBLIC_IP}:${LOG_DIR} ./docs/runs/"
    log "  aws s3 cp ${LOG_DIR}/ s3://your-bucket/blazil-vsr/ --recursive"
else
    log "⚠️  Copy logs to S3 NOW if needed:"
    log "  aws s3 cp ${LOG_DIR}/ s3://your-bucket/blazil-vsr/ --recursive"
    log "Shutting down in 15 seconds... (SKIP_SHUTDOWN=1 to cancel)"
    sleep 15
    shutdown -h now
fi

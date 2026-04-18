#!/usr/bin/env bash
# =============================================================================
# v0.4_nvme_oneshot.sh — Blazil v0.4 One-Shot Execution Script
#
# Instance  : AWS i4i.16xlarge (64 vCPU / 512 GiB / 4× 3.75 TB NVMe)
# Purpose   : Format → Mount → Kernel Tune → TigerBeetle → Build → Bench → Shutdown
#
# Usage (as root, or paste into EC2 User Data):
#   chmod +x scripts/v0.4_nvme_oneshot.sh
#   sudo bash scripts/v0.4_nvme_oneshot.sh
#
# ⚠️  EPHEMERAL STORAGE WARNING — READ BEFORE USE ⚠️
#   /dev/nvme1n1 → /dev/nvme4n1 are INSTANCE STORE volumes.
#   ALL DATA IS PERMANENTLY LOST when the instance stops or terminates.
#   NEVER put on these volumes:
#     - Source code / git repositories
#     - .env files, secrets, private keys
#     - Anything you cannot reproduce
#   Keep persistent data on EBS (root volume or attached volumes).
#
# Volume layout:
#   /dev/nvme1n1 → /mnt/data1  — TigerBeetle data (--directory)
#   /dev/nvme2n1 → /mnt/data2  — Blazil logs + Prometheus metrics
#   /dev/nvme3n1 → /mnt/data3  — Bench workload / replica node A
#   /dev/nvme4n1 → /mnt/data4  — Bench workload / replica node B
#
# Optional env overrides:
#   SHARDS        — shard count for main bench (default: 32)
#   DURATION      — bench duration in seconds (default: 60)
#   METRICS_PORT  — WebSocket dashboard port (default: 9090)
#   TB_VERSION    — TigerBeetle Docker image tag (default: 0.16.78)
#   REPO_DIR      — path to Blazil repo (default: /opt/blazil)
#   SKIP_SHUTDOWN — set to 1 to skip auto-shutdown at end (default: 0)
# =============================================================================

set -euo pipefail

# ── Colour helpers ────────────────────────────────────────────────────────────
RED='\033[0;31m'; GRN='\033[0;32m'; YLW='\033[1;33m'
BLU='\033[0;34m'; CYN='\033[0;36m'; NC='\033[0m'

log()  { echo -e "${GRN}[oneshot]${NC} $*" | tee -a "$LOG_FILE"; }
warn() { echo -e "${YLW}[warn]${NC}   $*" | tee -a "$LOG_FILE"; }
err()  { echo -e "${RED}[error]${NC}  $*" | tee -a "$LOG_FILE" >&2; }
hdr()  {
    echo -e "\n${BLU}╔══════════════════════════════════════════════════════╗${NC}" | tee -a "$LOG_FILE"
    echo -e "${BLU}║  $*${NC}" | tee -a "$LOG_FILE"
    echo -e "${BLU}╚══════════════════════════════════════════════════════╝${NC}" | tee -a "$LOG_FILE"
}
banner() {
    echo -e "${CYN}"
    echo "  ██████╗ ██╗      █████╗ ███████╗██╗██╗     "
    echo "  ██╔══██╗██║     ██╔══██╗╚══███╔╝██║██║     "
    echo "  ██████╔╝██║     ███████║  ███╔╝ ██║██║     "
    echo "  ██╔══██╗██║     ██╔══██║ ███╔╝  ██║██║     "
    echo "  ██████╔╝███████╗██║  ██║███████╗██║███████╗"
    echo "  ╚═════╝ ╚══════╝╚═╝  ╚═╝╚══════╝╚═╝╚══════╝"
    echo -e "${NC}"
    echo -e "  ${YLW}v0.4 — i4i.16xlarge One-Shot Execution${NC}"
    echo ""
}

# ── Config ────────────────────────────────────────────────────────────────────
SHARDS="${SHARDS:-32}"
DURATION="${DURATION:-60}"
METRICS_PORT="${METRICS_PORT:-9090}"
TB_VERSION="${TB_VERSION:-0.16.78}"
REPO_DIR="${REPO_DIR:-/opt/blazil}"
SKIP_SHUTDOWN="${SKIP_SHUTDOWN:-0}"

TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
LOG_DIR="/mnt/data2/blazil-logs"      # on NVMe data2 (logs volume)
FALLBACK_LOG="/tmp/blazil-oneshot-${TIMESTAMP}.log"
LOG_FILE="$FALLBACK_LOG"              # will be updated after data2 is mounted

# NVMe device → mount point → role mapping
declare -A NVME_MOUNT=(
    [nvme1n1]="/mnt/data1"
    [nvme2n1]="/mnt/data2"
    [nvme3n1]="/mnt/data3"
    [nvme4n1]="/mnt/data4"
)
declare -A NVME_ROLE=(
    [nvme1n1]="TigerBeetle data (--directory)"
    [nvme2n1]="Blazil logs + Prometheus metrics"
    [nvme3n1]="Bench workload / replica node A"
    [nvme4n1]="Bench workload / replica node B"
)

TB_DATA_DIR="/mnt/data1/tigerbeetle"
PROMETHEUS_DIR="/mnt/data2/prometheus"
BENCH_SCRATCH_A="/mnt/data3/bench"
BENCH_SCRATCH_B="/mnt/data4/bench"

# ── Pre-flight ────────────────────────────────────────────────────────────────
[[ $EUID -ne 0 ]] && { echo -e "${RED}[error]${NC} Must run as root (sudo $0)"; exit 1; }

touch "$LOG_FILE"
banner | tee "$LOG_FILE"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 0 — EPHEMERAL STORAGE WARNING
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 0 — ⚠️  EPHEMERAL STORAGE WARNING"

cat <<'WARN' | tee -a "$LOG_FILE"

  ┌─────────────────────────────────────────────────────────────┐
  │  ⚠️   INSTANCE STORE = EPHEMERAL STORAGE  ⚠️                 │
  │                                                             │
  │  /dev/nvme1n1 → /dev/nvme4n1 are LOCAL NVMe SSDs.          │
  │  Data is PERMANENTLY LOST when the instance:               │
  │    • Stops (not hibernate)                                  │
  │    • Terminates                                             │
  │    • Has a hardware failure                                 │
  │                                                             │
  │  ❌ NEVER store on these volumes:                           │
  │     - Source code / git repos                               │
  │     - .env files, secrets, private keys, SSH keys          │
  │     - TLS certificates                                      │
  │     - Anything not reproducible from EBS/S3                 │
  │                                                             │
  │  ✅ Safe to store (ephemeral, reproducible):                │
  │     - TigerBeetle data files (bench only, not prod)         │
  │     - Bench logs and metrics                                │
  │     - Build artifacts (rebuilt from source)                 │
  └─────────────────────────────────────────────────────────────┘

WARN

log "Acknowledged. Continuing in 3 seconds..."
sleep 3

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 1 — DEPENDENCY CHECK
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 1 — Dependency Check"

MISSING=()
for cmd in xfs_mkfs mkfs.xfs docker git cargo curl; do
    if ! command -v "$cmd" &>/dev/null; then
        # mkfs.xfs is the actual binary, xfs_mkfs is alias
        [[ "$cmd" == "xfs_mkfs" ]] && continue
        MISSING+=("$cmd")
    fi
done

# xfsprogs
if ! command -v mkfs.xfs &>/dev/null; then
    log "Installing xfsprogs..."
    apt-get update -qq && apt-get install -y -qq xfsprogs
fi

# nvme-cli (for model detection)
if ! command -v nvme &>/dev/null; then
    apt-get install -y -qq nvme-cli 2>/dev/null || true
fi

if [[ ${#MISSING[@]} -gt 0 ]]; then
    err "Missing required tools: ${MISSING[*]}"
    err "Install them and rerun. On Ubuntu: apt-get install -y ${MISSING[*]}"
    exit 1
fi
log "All dependencies present"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 2 — DETECT NVMe DEVICES
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 2 — NVMe Device Detection"

log "Scanning for instance store NVMe devices..."

DETECTED=()
for dev in nvme1n1 nvme2n1 nvme3n1 nvme4n1; do
    DEVPATH="/dev/$dev"
    if [[ ! -b "$DEVPATH" ]]; then
        warn "$DEVPATH not found — skipping (may be unavailable on this instance type)"
        continue
    fi

    # Skip if this device is currently mounted as root
    root_dev="$(findmnt -n -o SOURCE / 2>/dev/null | sed 's/p[0-9]*$//' || true)"
    if [[ "$DEVPATH" == "$root_dev"* ]]; then
        warn "$DEVPATH appears to be root device — skipping"
        continue
    fi

    # Warn if already mounted (idempotent re-run protection)
    if mount | grep -q "^${DEVPATH} "; then
        MOUNT_PT="$(mount | grep "^${DEVPATH} " | awk '{print $3}')"
        warn "$DEVPATH already mounted at $MOUNT_PT — will skip format, proceed with tuning"
    fi

    DETECTED+=("$dev")
    log "  Found: $DEVPATH → ${NVME_MOUNT[$dev]} (${NVME_ROLE[$dev]})"
done

if [[ ${#DETECTED[@]} -eq 0 ]]; then
    err "No instance store NVMe devices found (nvme1n1-nvme4n1)."
    err "Check that you are on an i4i instance type with instance store volumes attached."
    exit 1
fi

log "Detected ${#DETECTED[@]}/4 NVMe instance store volumes"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 3 — FORMAT (XFS) + MOUNT
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 3 — Format XFS + Mount"

for dev in "${DETECTED[@]}"; do
    DEVPATH="/dev/$dev"
    MNTPT="${NVME_MOUNT[$dev]}"

    # Skip format if already mounted
    if mount | grep -q "^${DEVPATH} "; then
        log "  $dev: already mounted, skipping format"
        continue
    fi

    log "  Formatting $DEVPATH → XFS..."
    # XFS options tuned for TigerBeetle / database workloads:
    #   -f          : force (wipe any existing fs signature)
    #   -d agcount  : 64 allocation groups = one per vCPU thread for parallel writes
    #   -l size=256m: large log for high-concurrency workload
    #   -i size=512  : 512-byte inode for extended attributes
    mkfs.xfs \
        -f \
        -d agcount=64 \
        -l size=256m \
        -i size=512 \
        "$DEVPATH" \
        2>&1 | tee -a "$LOG_FILE"

    log "  Mounting $DEVPATH → $MNTPT"
    mkdir -p "$MNTPT"
    # Mount options:
    #   noatime     : skip access-time writes (huge win for DB sequential I/O)
    #   nodiratime  : same for directories
    #   nobarrier   : TigerBeetle manages its own fsync; skip redundant barriers
    #   logbsize=256k: match the mkfs log buffer size
    #   allocsize=64m: aggressive pre-allocation for large sequential writes
    mount -t xfs \
        -o noatime,nodiratime,nobarrier,logbsize=256k,allocsize=64m \
        "$DEVPATH" \
        "$MNTPT"

    log "  ✓ $dev mounted at $MNTPT"
done

# Verify all mounts
log "Current NVMe mounts:"
for dev in "${DETECTED[@]}"; do
    MNTPT="${NVME_MOUNT[$dev]}"
    df -h "$MNTPT" 2>/dev/null | tail -1 | tee -a "$LOG_FILE" || warn "$MNTPT not mounted"
done

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 4 — KERNEL I/O TUNING (Mechanical Sympathy for TigerBeetle)
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 4 — Kernel I/O Tuning (Mechanical Sympathy)"

for dev in "${DETECTED[@]}"; do
    # Strip the partition suffix to get the block device name (nvme1n1 → nvme1n1)
    BLKDEV="$dev"
    SYS_BLOCK="/sys/block/${BLKDEV}"

    if [[ ! -d "$SYS_BLOCK" ]]; then
        warn "  /sys/block/$BLKDEV not found — skipping scheduler tuning"
        continue
    fi

    # ── I/O scheduler: none (bypass kernel queue entirely for NVMe) ───────────
    # NVMe drives have internal multi-queue hardware. Adding a kernel scheduler
    # on top only adds latency. "none" = pass-through to hardware queues.
    # TigerBeetle does its own I/O ordering via VSR protocol.
    SCHED_FILE="$SYS_BLOCK/queue/scheduler"
    if [[ -f "$SCHED_FILE" ]]; then
        if echo "none" > "$SCHED_FILE" 2>/dev/null; then
            log "  $dev: I/O scheduler → none (NVMe pass-through)"
        elif echo "mq-deadline" > "$SCHED_FILE" 2>/dev/null; then
            # Fallback: mq-deadline is next best for latency-sensitive DB workloads
            warn "  $dev: 'none' unavailable → mq-deadline (still good for TB)"
        else
            warn "  $dev: could not set scheduler (kernel may not support it)"
        fi
    fi

    # ── read_ahead_kb = 0 (let TigerBeetle control its own read patterns) ─────
    # TB uses direct mmap and sequential log reads internally.
    # OS read-ahead would waste memory bandwidth on already-cached data.
    # Setting to 0 tells the kernel: "do not speculatively read ahead".
    RA_FILE="$SYS_BLOCK/queue/read_ahead_kb"
    if [[ -f "$RA_FILE" ]]; then
        echo 0 > "$RA_FILE"
        log "  $dev: read_ahead_kb → 0 (TigerBeetle controls read patterns)"
    fi

    # ── nr_requests: max queue depth for NVMe ─────────────────────────────────
    # i4i NVMe supports 1024 hardware queue entries. Setting kernel side to
    # match avoids queue saturation under sustained bench load.
    NR_REQ="$SYS_BLOCK/queue/nr_requests"
    if [[ -f "$NR_REQ" ]]; then
        echo 1024 > "$NR_REQ" 2>/dev/null || true
        log "  $dev: nr_requests → 1024"
    fi

    # ── rq_affinity = 2: keep completion on the issuing CPU ──────────────────
    # Reduces cross-CPU cache invalidation for the shard threads that own each
    # pipeline's I/O path.
    RQ_AFF="$SYS_BLOCK/queue/rq_affinity"
    if [[ -f "$RQ_AFF" ]]; then
        echo 2 > "$RQ_AFF" 2>/dev/null || true
        log "  $dev: rq_affinity → 2 (completion on issuing CPU)"
    fi

    # ── write_cache: keep enabled (NVMe has battery-backed cache on i4i) ──────
    # i4i NVMe is local SSD — writes go to DRAM buffer then NVMe cells.
    # TB's fsync() still works correctly with cache enabled.
    WC_FILE="$SYS_BLOCK/queue/write_cache"
    if [[ -f "$WC_FILE" ]]; then
        echo "write back" > "$WC_FILE" 2>/dev/null || true
        log "  $dev: write_cache → write back (battery-backed NVMe)"
    fi
done

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 5 — DIRECTORY STRUCTURE + OS TUNING
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 5 — Directory Layout + OS Tuning"

# TigerBeetle data (data1)
mkdir -p "$TB_DATA_DIR"
chmod 700 "$TB_DATA_DIR"
log "TB data dir     : $TB_DATA_DIR"

# Logs + metrics (data2) — switch LOG_FILE here so subsequent logs go to NVMe
mkdir -p "$LOG_DIR" "$PROMETHEUS_DIR"
cp "$FALLBACK_LOG" "$LOG_DIR/oneshot-${TIMESTAMP}.log" 2>/dev/null || true
LOG_FILE="$LOG_DIR/oneshot-${TIMESTAMP}.log"
log "Log dir         : $LOG_DIR"
log "Prometheus dir  : $PROMETHEUS_DIR"

# Bench scratch dirs (data3, data4)
mkdir -p "$BENCH_SCRATCH_A" "$BENCH_SCRATCH_B"
log "Bench scratch A : $BENCH_SCRATCH_A"
log "Bench scratch B : $BENCH_SCRATCH_B"

# ── OS-level tuning ───────────────────────────────────────────────────────────
log "Applying OS tuning..."

# Ulimits
ulimit -n 1048576 2>/dev/null || warn "ulimit -n: set via limits.conf on next login"
sysctl -w fs.file-max=1048576        > /dev/null
sysctl -w fs.nr_open=1048576         > /dev/null

# TCP stack (128 MB buffers, BBR)
sysctl -w net.core.rmem_max=134217728      > /dev/null
sysctl -w net.core.wmem_max=134217728      > /dev/null
sysctl -w net.core.rmem_default=134217728  > /dev/null
sysctl -w net.core.wmem_default=134217728  > /dev/null
sysctl -w net.core.netdev_max_backlog=250000 > /dev/null
sysctl -w net.ipv4.tcp_tw_reuse=1          > /dev/null
sysctl -w net.ipv4.tcp_fin_timeout=15      > /dev/null
modprobe tcp_bbr 2>/dev/null && {
    sysctl -w net.ipv4.tcp_congestion_control=bbr > /dev/null
    sysctl -w net.core.default_qdisc=fq            > /dev/null
    log "BBR congestion control: enabled"
} || warn "tcp_bbr unavailable"

# CPU: performance governor + disable C2+ states
if [[ -d /sys/devices/system/cpu/cpu0/cpufreq ]]; then
    echo performance | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
    log "CPU governor: performance (64 cores)"
fi
for cpu_dir in /sys/devices/system/cpu/cpu*/cpuidle/state*/; do
    state_name="$(cat "${cpu_dir}name" 2>/dev/null || true)"
    case "$state_name" in C2|C3|C6|C7) echo 1 > "${cpu_dir}disable" 2>/dev/null || true ;; esac
done
log "Deep c-states (C2+): disabled"

# Huge pages (16 GB for TB mmap)
echo 8192 > /proc/sys/vm/nr_hugepages 2>/dev/null || warn "hugepages: non-fatal"
log "Hugepages: 8192 × 2 MB reserved"

# NUMA balancing off (single socket i4i)
echo 0 > /proc/sys/kernel/numa_balancing 2>/dev/null || true
log "NUMA balancing: disabled"

# Transparent huge pages: madvise (let TB opt in per mmap region)
echo madvise > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true
log "THP: madvise mode"

log "OS tuning: complete"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 6 — TIGERBEETLE SETUP
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 6 — TigerBeetle"

TB_DATA_FILE="$TB_DATA_DIR/0_0.tigerbeetle"
TB_PORT=3000

if ! command -v tigerbeetle &>/dev/null; then
    log "Downloading TigerBeetle binary..."
    TB_BIN="/usr/local/bin/tigerbeetle"
    curl -fsSL \
        "https://github.com/tigerbeetle/tigerbeetle/releases/download/${TB_VERSION}/tigerbeetle-x86_64-linux.zip" \
        -o /tmp/tb.zip
    unzip -qo /tmp/tb.zip -d /tmp/tb-extract
    install -m 755 /tmp/tb-extract/tigerbeetle "$TB_BIN"
    rm -rf /tmp/tb.zip /tmp/tb-extract
    log "TigerBeetle $TB_VERSION installed at $TB_BIN"
fi

TB_VERSION_ACTUAL="$(tigerbeetle version 2>/dev/null | head -1 || echo unknown)"
log "TigerBeetle version: $TB_VERSION_ACTUAL"

# Format data file if not present
if [[ ! -f "$TB_DATA_FILE" ]]; then
    log "Formatting TigerBeetle data file..."
    tigerbeetle format \
        --cluster=0 \
        --replica=0 \
        --replica-count=1 \
        "$TB_DATA_FILE" \
        2>&1 | tee -a "$LOG_FILE"
    log "TB data file: $TB_DATA_FILE ($(du -sh "$TB_DATA_FILE" | cut -f1))"
else
    log "TB data file already exists — reusing"
fi

# Start TigerBeetle
log "Starting TigerBeetle on :$TB_PORT..."
ulimit -n 1048576
nohup tigerbeetle start \
    --addresses="0.0.0.0:${TB_PORT}" \
    "$TB_DATA_FILE" \
    > "$LOG_DIR/tigerbeetle-${TIMESTAMP}.log" 2>&1 &
TB_PID=$!
log "TigerBeetle PID: $TB_PID"

# Wait for TB to be ready
log "Waiting for TigerBeetle to accept connections..."
for i in $(seq 1 30); do
    if timeout 1 bash -c "echo > /dev/tcp/127.0.0.1/${TB_PORT}" 2>/dev/null; then
        log "TigerBeetle ready after ${i}s ✓"
        break
    fi
    if [[ $i -eq 30 ]]; then
        err "TigerBeetle did not become ready in 30s"
        err "Check log: $LOG_DIR/tigerbeetle-${TIMESTAMP}.log"
        exit 1
    fi
    sleep 1
done

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 7 — BUILD BLAZIL BENCH (release)
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 7 — Build Blazil (release)"

if [[ ! -d "$REPO_DIR" ]]; then
    err "Repo not found at $REPO_DIR"
    err "Clone the repo first: git clone <url> $REPO_DIR"
    err "⚠️  Clone to EBS (root volume), NOT to /mnt/dataN (ephemeral NVMe)"
    exit 1
fi

cd "$REPO_DIR"

# Sanity check: make sure user didn't accidentally clone to ephemeral volume
for mount_pt in /mnt/data1 /mnt/data2 /mnt/data3 /mnt/data4; do
    if [[ "$REPO_DIR" == "${mount_pt}"* ]]; then
        warn "╔══════════════════════════════════════════════════════╗"
        warn "║  ⚠️   REPO IS ON EPHEMERAL NVMe — DATA WILL BE LOST  ║"
        warn "║  Move repo to /opt/blazil (EBS) before next run!    ║"
        warn "╚══════════════════════════════════════════════════════╝"
        break
    fi
done

log "Building blazil-bench --release..."
BUILD_START=$(date +%s)
cargo build --release --bin blazil-bench 2>&1 | tee -a "$LOG_FILE"
BUILD_END=$(date +%s)
log "Build complete in $((BUILD_END - BUILD_START))s"

BENCH_BIN="$REPO_DIR/target/release/blazil-bench"
[[ -x "$BENCH_BIN" ]] || { err "Build artifact not found: $BENCH_BIN"; exit 1; }

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 8 — WARMUP
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 8 — Warmup (8 shards × 5s)"

log "Warming TigerBeetle page cache and Rust allocator..."
BLAZIL_TB_ADDRESS="127.0.0.1:${TB_PORT}" \
    "$BENCH_BIN" \
    --scenario sharded-tb \
    --shards 8 \
    --duration 5 \
    2>&1 | tee -a "$LOG_FILE" || warn "Warmup exited non-zero (non-fatal)"
log "Warmup: done"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 9 — MAIN BENCHMARK (32 shards × 60s, live dashboard)
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 9 — Main Bench (${SHARDS} shards × ${DURATION}s)"

log "Dashboard WebSocket: ws://$(curl -s http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null || hostname -I | awk '{print $1}'):${METRICS_PORT}/ws"
log ""

BENCH_START=$(date +%s)
BLAZIL_TB_ADDRESS="127.0.0.1:${TB_PORT}" \
    "$BENCH_BIN" \
    --scenario sharded-tb \
    --shards "$SHARDS" \
    --duration "$DURATION" \
    --metrics-port "$METRICS_PORT" \
    2>&1 | tee -a "$LOG_FILE"
BENCH_END=$(date +%s)
log "Main bench finished in $((BENCH_END - BENCH_START))s"

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 10 — SHARD SCALING SWEEP (1 → 2 → 4 → 8 → 16 → 32 shards)
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 10 — Shard Scaling Sweep"

SWEEP_LOG="$LOG_DIR/sweep-${TIMESTAMP}.log"
log "Sweep: 1/2/4/8/16/32 shards × 30s each → building linear scaling curve"
log "Sweep log: $SWEEP_LOG"

for N in 1 2 4 8 16 32; do
    hdr "  Sweep: $N shards"
    BLAZIL_TB_ADDRESS="127.0.0.1:${TB_PORT}" \
        "$BENCH_BIN" \
        --scenario sharded-tb \
        --shards "$N" \
        --duration 30 \
        2>&1 | tee -a "$LOG_FILE" "$SWEEP_LOG"
    sleep 3   # let TB flush between runs
done

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 11 — COLLECT RESULTS
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 11 — Results Summary"

# Extract peak TPS from logs
PEAK_TPS="$(grep -oP '(?<=max_tps":)\d+' "$LOG_FILE" | sort -n | tail -1 || echo "N/A")"
AVG_TPS="$(grep -oP '(?<=avg_tps":)\d+' "$LOG_FILE" | sort -n | tail -1 || echo "N/A")"
CONSISTENCY="$(grep -oP '(?<=consistency":)[0-9.]+' "$LOG_FILE" | tail -1 || echo "N/A")"

log "╔═══════════════════════════════════════╗"
log "║  Blazil v0.4 — Bench Results          ║"
log "║  Instance : i4i.16xlarge (64 vCPU)    ║"
log "║  Shards   : $SHARDS × ${DURATION}s                 ║"
log "║  Peak TPS : $PEAK_TPS                  ║"
log "║  Avg  TPS : $AVG_TPS                   ║"
log "║  Consistency: ${CONSISTENCY}%              ║"
log "╚═══════════════════════════════════════╝"
log ""
log "Full log  : $LOG_FILE"
log "Sweep log : $SWEEP_LOG"
log "TB log    : $LOG_DIR/tigerbeetle-${TIMESTAMP}.log"

# Kill TB gracefully
log "Stopping TigerBeetle (PID $TB_PID)..."
kill "$TB_PID" 2>/dev/null || true
wait "$TB_PID" 2>/dev/null || true
log "TigerBeetle stopped"

# Sync logs to EBS / stdout before shutdown
sync

# ══════════════════════════════════════════════════════════════════════════════
# PHASE 12 — AUTO-SHUTDOWN
# ══════════════════════════════════════════════════════════════════════════════
hdr "PHASE 12 — Auto-Shutdown"

if [[ "$SKIP_SHUTDOWN" == "1" ]]; then
    log "SKIP_SHUTDOWN=1 — skipping shutdown. Instance remains running."
    log "Remember to terminate manually to avoid EC2 charges!"
    log ""
    log "Results are on /mnt/data2 (ephemeral). Copy to S3 before stopping:"
    log "  aws s3 cp $LOG_FILE s3://your-bucket/blazil-results/"
    log "  aws s3 cp $SWEEP_LOG s3://your-bucket/blazil-results/"
else
    log "Shutting down in 10 seconds..."
    log "(Set SKIP_SHUTDOWN=1 to prevent this)"
    log ""
    log "⚠️  Copy logs to S3 NOW if you want to keep them:"
    log "  aws s3 cp $LOG_FILE s3://your-bucket/blazil-results/"
    log "  aws s3 cp $SWEEP_LOG s3://your-bucket/blazil-results/"
    sleep 10
    log "Initiating shutdown (EC2 Terminate if Shutdown Behavior = Terminate)"
    shutdown -h now
fi

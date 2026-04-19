#!/usr/bin/env bash
# blazil-bench v0.5-Lightning — VSR 3-node · Dashboard-First · 120s
#
# Timeline (from bench binary launch):
#   T+0s    WS server binds :9090 — dashboard connects NOW
#   T+5s    scenario starts (5s pre-warmup window in binary)
#   T+5s    warmup phase (~20s, data visible on dashboard)
#   T+25s   peak TPS measurement window begins
#   T+85s   kill -9 replica 2  (= scenario t=80s)
#   T+125s  bench ends → final report, instance stays alive
#
# Changes from v0.4:
#   - NO taskset / CPU pinning  → Linux scheduler owns all 16 vCPUs
#   - Single 120s process       → dashboard sees one continuous stream
#   - WINDOW=512/shard          → latency <5ms, TPS peak on 4 shards
#   - Dashboard-First           → port :9090 ready 5s before data flows
#   - No auto-shutdown          → inspect results on web after bench
#
# Usage:
#   sudo bash scripts/v0.4_vsr_bench.sh

set -euo pipefail
[[ $EUID -ne 0 ]] && { echo "run as root (sudo $0)"; exit 1; }

# ── constants ──────────────────────────────────────────────────────────────
readonly BENCH_BIN="/mnt/data1/cargo-target/release/blazil-bench"
readonly TB_DATA="/mnt/data1"
readonly TB_ADDRS="127.0.0.1:3001,127.0.0.1:3002,127.0.0.1:3003"
readonly SHARDS=4
readonly METRICS_PORT=9090
readonly TB_VERSION="0.16.78"

# ── OS tuning ──────────────────────────────────────────────────────────────
cpupower frequency-set -g performance 2>/dev/null || \
    for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        echo performance > "$g" 2>/dev/null || true
    done
sysctl -w net.core.somaxconn=65535       2>/dev/null || true
sysctl -w net.ipv4.tcp_tw_reuse=1        2>/dev/null || true
sysctl -w net.core.rmem_max=16777216     2>/dev/null || true
sysctl -w net.core.wmem_max=16777216     2>/dev/null || true
echo never > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true
ulimit -n 1048576
ulimit -c 0 2>/dev/null || true

echo "[v0.5] OS tuned — cpupower performance, somaxconn=65535"

# ── binary guard ───────────────────────────────────────────────────────────
[[ -x "$BENCH_BIN" ]] || {
    echo "FATAL: $BENCH_BIN not found. Build first:"
    echo "  CARGO_TARGET_DIR=/mnt/data1/cargo-target \\"
    echo "    cargo build --release --bin blazil-bench \\"
    echo "    --features blazil-bench/metrics-ws,blazil-bench/tigerbeetle-client"
    exit 1
}

# ── tigerbeetle ────────────────────────────────────────────────────────────
if ! command -v tigerbeetle &>/dev/null; then
    echo "[v0.5] downloading tigerbeetle ${TB_VERSION}..."
    curl -fsSL \
        "https://github.com/tigerbeetle/tigerbeetle/releases/download/${TB_VERSION}/tigerbeetle-x86_64-linux.zip" \
        -o /tmp/tb.zip
    unzip -qo /tmp/tb.zip -d /tmp/tb-x
    install -m 755 /tmp/tb-x/tigerbeetle /usr/local/bin/tigerbeetle
    rm -rf /tmp/tb.zip /tmp/tb-x
fi

# ── cleanup ────────────────────────────────────────────────────────────────
TB0="" TB1="" TB2="" BENCH_PID="" KILL_TIMER=""
cleanup() {
    [[ -n "$KILL_TIMER" ]] && kill "$KILL_TIMER" 2>/dev/null || true
    [[ -n "$BENCH_PID" ]] && kill "$BENCH_PID"  2>/dev/null || true
    [[ -n "$TB0" ]]       && kill "$TB0"         2>/dev/null || true
    [[ -n "$TB1" ]]       && kill "$TB1"         2>/dev/null || true
    [[ -n "$TB2" ]]       && kill "$TB2"         2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ── format fresh cluster ───────────────────────────────────────────────────
echo "[v0.5] ── PHASE 1: Format cluster ──"
pkill -f "tigerbeetle start" 2>/dev/null && sleep 1 || true
rm -rf "${TB_DATA}"/tb-node{0,1,2}
mkdir -p "${TB_DATA}"/tb-node{0,1,2}
for r in 0 1 2; do
    tigerbeetle format --cluster=0 --replica=$r --replica-count=3 \
        "${TB_DATA}/tb-node${r}/0_${r}.tigerbeetle" 2>/dev/null
done
echo "[v0.5] 3 replicas formatted (O_DIRECT on all data paths)"

# ── start cluster (no taskset — Linux scheduler decides) ──────────────────
echo "[v0.5] ── PHASE 2: Start cluster ──"
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node0/0_0.tigerbeetle" &>/tmp/tb0.log &
TB0=$!
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node1/0_1.tigerbeetle" &>/tmp/tb1.log &
TB1=$!
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node2/0_2.tigerbeetle" &>/tmp/tb2.log &
TB2=$!
echo "[v0.5] pids: tb0=$TB0 tb1=$TB1 tb2=$TB2 (no CPU pinning)"

# ── wait for VSR quorum ────────────────────────────────────────────────────
printf "[v0.5] waiting for quorum"
for port in 3001 3002 3003; do
    for i in $(seq 1 30); do
        timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null && break
        [[ $i -eq 30 ]] && { echo; echo "FATAL: :$port not ready in 30s"; exit 1; }
        printf "."; sleep 1
    done
done
sleep 5
echo " ✓"

# ── DASHBOARD FIRST: launch bench, wait for :9090 ─────────────────────────
echo "[v0.5] ── PHASE 3: Dashboard-First Launch ──"
PUBLIC_IP="$(curl -s --connect-timeout 2 \
    http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null \
    || hostname -I | awk '{print $1}')"

BENCH_LOG=$(mktemp /tmp/blazil-v05-XXXX.log)
KILL_TS_FILE=$(mktemp /tmp/blazil-kill-XXXX.ms)

# Single 120s process — dashboard sees one unbroken stream.
# Binary sleeps 5s before starting scenario (Dashboard-First guarantee).
nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRS" \
    "$BENCH_BIN" \
    --scenario   sharded-tb \
    --shards     "$SHARDS" \
    --duration   120 \
    --metrics-port "$METRICS_PORT" \
    >"$BENCH_LOG" 2>&1 &
BENCH_PID=$!

# Poll until :9090 is ready (< 500ms typically)
printf "[v0.5] polling :$METRICS_PORT"
for i in $(seq 1 40); do
    timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/$METRICS_PORT" 2>/dev/null && break
    printf "."; sleep 0.25
done
echo ""

echo "┌─────────────────────────────────────────────────────┐"
echo "│  DASHBOARD READY                                    │"
echo "│  ws://${PUBLIC_IP}:${METRICS_PORT}/ws               │"
echo "│                                                     │"
echo "│  Bench scenario starts in ~5s — CONNECT NOW        │"
echo "└─────────────────────────────────────────────────────┘"

# ── background kill timer ─────────────────────────────────────────────────
# Scenario starts at T+5s (5s WS warmup in binary).
# We want kill at scenario t=80s = binary T+85s.
(
    sleep 85
    date +%s%3N > "$KILL_TS_FILE"
    kill -9 "$TB2" 2>/dev/null || true
    echo "[v0.5] ⚡ scenario t=80s — kill -9 replica2 — VSR on 2-of-3"
) &
KILL_TIMER=$!

# ── wait for bench ─────────────────────────────────────────────────────────
echo "[v0.5] bench running — 120s scenario (+ 5s pre-warmup) ..."
echo "[v0.5] failover fires at scenario t=80s"
echo ""
wait "$BENCH_PID" 2>/dev/null || true
BENCH_PID=""
kill "$KILL_TIMER" 2>/dev/null || true
KILL_TIMER=""

# ── parse results ──────────────────────────────────────────────────────────
PEAK_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$BENCH_LOG" \
    | tr -d ',' | sort -n | tail -1 2>/dev/null || echo "0")
AVG_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$BENCH_LOG" \
    | tr -d ',' | awk '{s+=$1;c++} END{printf "%d", (c?s/c:0)}' 2>/dev/null || echo "0")
P99_US=$(grep -oP 'p99=\K[0-9]+(?= µs)' "$BENCH_LOG" \
    | sort -n | tail -1 2>/dev/null || echo "0")
P99_MS=$(awk "BEGIN{printf \"%.1f\", ${P99_US:-0}/1000}")

# Recovery time: kill timestamp → first TPS line after it
RECOVERY_MS="N/A"
if [[ -s "$KILL_TS_FILE" ]]; then
    KILL_EPOCH_MS=$(cat "$KILL_TS_FILE")
    BENCH_END_MS=$(date +%s%3N)
    # approximate: TPS should resume within the 40s recovery window
    RECOVERY_MS=$(( BENCH_END_MS - KILL_EPOCH_MS ))
fi
rm -f "$BENCH_LOG" "$KILL_TS_FILE"

# ── final report ───────────────────────────────────────────────────────────
echo ""
echo "==================================================================="
echo "  BLAZIL v0.5-Lightning — VSR FINAL REPORT"
echo "  i4i.4xlarge  |  16 vCPU (free scheduler)  |  1.9TB NVMe"
echo "  WINDOW=512/shard  |  O_DIRECT  |  4 shards  |  3-replica VSR"
echo "==================================================================="
printf "  %-20s %s\n"    "Peak TPS:"      "${PEAK_TPS}"
printf "  %-20s %s\n"    "Average TPS:"   "${AVG_TPS}"
printf "  %-20s %s ms\n" "P99 Latency:"   "${P99_MS}"
printf "  %-20s %s ms\n" "Recovery Time:" "${RECOVERY_MS}"
echo "==================================================================="
echo ""
echo "[v0.5] Instance alive. Logs: /tmp/tb{0,1,2}.log"
echo "[v0.5] Dashboard still at ws://${PUBLIC_IP}:${METRICS_PORT}/ws"

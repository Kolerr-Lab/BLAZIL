#!/usr/bin/env bash
# Blazil bench — 4 shards · WINDOW=1024 · 120s · VSR 3-node
# Auto-failover kill -9 replica2 at t=80s. No shutdown. No taskset.
set -euo pipefail

[[ $EUID -ne 0 ]] && { echo "run as root (sudo $0)"; exit 1; }

# ── constants (DO NOT CHANGE) ──────────────────────────────────────────────
readonly SHARDS=4
readonly DURATION=120
readonly FAILOVER_AT=80        # seconds into bench
readonly METRICS_PORT=9090
readonly TB_ADDRS="127.0.0.1:3001,127.0.0.1:3002,127.0.0.1:3003"
readonly TB_DATA="/mnt/data1"
readonly BENCH_BIN="/mnt/data1/cargo-target/release/blazil-bench"
readonly TB_VERSION="0.16.78"

# ── OS tuning ──────────────────────────────────────────────────────────────
cpupower frequency-set -g performance 2>/dev/null || \
    for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        echo performance > "$g" 2>/dev/null || true
    done
sysctl -w net.core.somaxconn=65535    2>/dev/null || true
sysctl -w net.ipv4.tcp_tw_reuse=1     2>/dev/null || true
echo never > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true
ulimit -n 1048576

echo "[bench] shards=$SHARDS window=1024 duration=${DURATION}s failover=t${FAILOVER_AT}s"

# ── binary guard ───────────────────────────────────────────────────────────
[[ -x "$BENCH_BIN" ]] || {
    echo "FATAL: $BENCH_BIN not found. Build:"
    echo "  CARGO_TARGET_DIR=/mnt/data1/cargo-target \\"
    echo "    cargo build --release --bin blazil-bench \\"
    echo "    --features blazil-bench/metrics-ws,blazil-bench/tigerbeetle-client"
    exit 1
}

# ── tigerbeetle ────────────────────────────────────────────────────────────
if ! command -v tigerbeetle &>/dev/null; then
    curl -fsSL \
        "https://github.com/tigerbeetle/tigerbeetle/releases/download/${TB_VERSION}/tigerbeetle-x86_64-linux.zip" \
        -o /tmp/tb.zip
    unzip -qo /tmp/tb.zip -d /tmp/tb-x
    install -m 755 /tmp/tb-x/tigerbeetle /usr/local/bin/tigerbeetle
    rm -rf /tmp/tb.zip /tmp/tb-x
fi

# ── cleanup ────────────────────────────────────────────────────────────────
TB0="" TB1="" TB2="" BENCH_PID="" FAILOVER_PID=""
cleanup() {
    [[ -n "$FAILOVER_PID" ]] && kill "$FAILOVER_PID" 2>/dev/null || true
    [[ -n "$BENCH_PID"    ]] && kill "$BENCH_PID"    2>/dev/null || true
    for p in "$TB0" "$TB1" "$TB2"; do
        [[ -n "$p" ]] && kill "$p" 2>/dev/null || true
    done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ── STEP 1: format fresh cluster ───────────────────────────────────────────
echo "[bench] ── step 1: format cluster ──"
pkill -f "tigerbeetle start" 2>/dev/null && sleep 1 || true
rm -rf "${TB_DATA}"/tb-node{0,1,2}
mkdir -p "${TB_DATA}"/tb-node{0,1,2}
for r in 0 1 2; do
    tigerbeetle format --cluster=0 --replica=$r --replica-count=3 \
        "${TB_DATA}/tb-node${r}/0_${r}.tigerbeetle" 2>/dev/null
done
echo "[bench] 3 replicas formatted (O_DIRECT)"

# ── STEP 2: start cluster ──────────────────────────────────────────────────
echo "[bench] ── step 2: start TigerBeetle cluster ──"
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node0/0_0.tigerbeetle" &>/tmp/tb0.log & TB0=$!
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node1/0_1.tigerbeetle" &>/tmp/tb1.log & TB1=$!
tigerbeetle start --addresses="$TB_ADDRS" \
    "${TB_DATA}/tb-node2/0_2.tigerbeetle" &>/tmp/tb2.log & TB2=$!
echo "[bench] tb0=$TB0 tb1=$TB1 tb2=$TB2"

# wait for all 3 ports
printf "[bench] waiting for VSR quorum"
for port in 3001 3002 3003; do
    for i in $(seq 1 30); do
        timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null && break
        [[ $i -eq 30 ]] && { echo; echo "FATAL: :$port not ready"; exit 1; }
        printf "."; sleep 1
    done
done
sleep 3
echo " ✓"

# ── STEP 3: start bench (WS server :9090 comes up first, then scenario) ───
echo "[bench] ── step 3: start bench — :${METRICS_PORT} ready in ~5s ──"
BENCH_LOG=$(mktemp /tmp/blazil-XXXX.log)
nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRS" \
    "$BENCH_BIN" \
    --scenario    sharded-tb \
    --shards      "$SHARDS" \
    --duration    "$DURATION" \
    --metrics-port "$METRICS_PORT" \
    >"$BENCH_LOG" 2>&1 &
BENCH_PID=$!

# wait for :9090
printf "[bench] polling :${METRICS_PORT}"
for i in $(seq 1 40); do
    timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/${METRICS_PORT}" 2>/dev/null && break
    printf "."; sleep 0.25
done
echo ""

PUBLIC_IP="$(curl -s --connect-timeout 2 \
    http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null \
    || hostname -I | awk '{print $1}')"
echo "┌──────────────────────────────────────────────┐"
echo "│  WS ready: ws://${PUBLIC_IP}:${METRICS_PORT}/ws"
echo "│  → Open dashboard and hit Connect now       │"
echo "│  Bench: ${DURATION}s | shards=${SHARDS} | window=1024    │"
echo "│  Failover kill at t=${FAILOVER_AT}s                 │"
echo "└──────────────────────────────────────────────┘"

# ── STEP 4: background failover timer ─────────────────────────────────────
# Binary has 5s WS warmup before scenario starts.
# kill replica2 at scenario t=80s = wall T+85s from bench launch.
KILL_TS_FILE=$(mktemp /tmp/blazil-kill-XXXX.ms)
(
    sleep $(( FAILOVER_AT + 5 ))
    date +%s%3N > "$KILL_TS_FILE"
    kill -9 "$TB2" 2>/dev/null || true
    TB2=""
    echo "[bench] ⚡ t=${FAILOVER_AT}s — kill -9 replica2 — VSR 2-of-3"
) &
FAILOVER_PID=$!

# ── STEP 5: wait for bench to finish ──────────────────────────────────────
echo "[bench] running ... (${DURATION}s + 5s pre-warmup)"
wait "$BENCH_PID" 2>/dev/null || true
BENCH_PID=""
kill "$FAILOVER_PID" 2>/dev/null || true
FAILOVER_PID=""

# ── STEP 6: parse + print report ──────────────────────────────────────────
PEAK_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$BENCH_LOG" \
    | tr -d ',' | sort -n | tail -1 2>/dev/null || echo "0")
AVG_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$BENCH_LOG" \
    | tr -d ',' | awk '{s+=$1;c++} END{printf "%d",(c?s/c:0)}')
P99_US=$(grep -oP 'p99=\K[0-9]+(?= µs)' "$BENCH_LOG" \
    | sort -n | tail -1 2>/dev/null || echo "0")
P99_MS=$(awk "BEGIN{printf \"%.1f\",${P99_US:-0}/1000}")

RECOVERY_MS="N/A"
if [[ -s "$KILL_TS_FILE" ]]; then
    KILL_EPOCH=$(cat "$KILL_TS_FILE")
    BENCH_END=$(date +%s%3N)
    RECOVERY_MS=$(( BENCH_END - KILL_EPOCH ))
fi
rm -f "$BENCH_LOG" "$KILL_TS_FILE"

echo ""
echo "============================================================"
echo "  BLAZIL — VSR BENCH REPORT"
echo "  4 shards | WINDOW=1024/shard | 3-node VSR | O_DIRECT"
echo "============================================================"
printf "  %-20s %s\n"    "Peak TPS:"      "$PEAK_TPS"
printf "  %-20s %s\n"    "Average TPS:"   "$AVG_TPS"
printf "  %-20s %s ms\n" "P99 Latency:"   "$P99_MS"
printf "  %-20s %s ms\n" "Recovery Time:" "$RECOVERY_MS"
echo "============================================================"
echo ""
echo "[bench] done — instance alive"
echo "[bench] TB logs: /tmp/tb0.log /tmp/tb1.log /tmp/tb2.log"

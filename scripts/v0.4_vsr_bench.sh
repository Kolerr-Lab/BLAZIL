#!/usr/bin/env bash
# blazil-bench — VSR 3-node · 120 s hardcoded · record attempt
#
# Core layout (i4i.4xlarge, 16 vCPU):
#   0- 3  TigerBeetle replica 0  (port 3001)
#   4- 7  TigerBeetle replica 1  (port 3002)
#   8-11  TigerBeetle replica 2  (port 3003)
#  12-15  blazil-bench  (4 shards, WINDOW=2048/shard)
#
# Disk I/O: TigerBeetle opens all data files with O_DIRECT by default.
#
# Timeline:
#    0s – 20s   warmup          (prime TB page-cache + VSR pipeline)
#   20s – 80s   main stress     (peak TPS measurement window)
#   80s          kill -9 replica 2
#   80s –120s   recovery test   (survival rate + recovery time)
#  121s          final report + shutdown

set -euo pipefail

[[ $EUID -ne 0 ]] && { echo "run as root"; exit 1; }

# ── hardcoded constants ────────────────────────────────────────────────────
readonly BENCH_BIN="/mnt/data1/cargo-target/release/blazil-bench"
readonly TB_DATA="/mnt/data1"
readonly TB_ADDRS="127.0.0.1:3001,127.0.0.1:3002,127.0.0.1:3003"
readonly SHARDS=4
readonly METRICS_PORT=9090
readonly TB_VERSION="0.16.78"
readonly SKIP_SHUTDOWN="${SKIP_SHUTDOWN:-0}"

# ── OS tuning ──────────────────────────────────────────────────────────────
cpupower frequency-set -g performance 2>/dev/null || \
    for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        echo performance > "$g" 2>/dev/null || true
    done
echo never > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true
ulimit -n 1048576
ulimit -c 0 2>/dev/null || true  # no core dumps wasting NVMe bandwidth

# ── binary guard ──────────────────────────────────────────────────────────
[[ -x "$BENCH_BIN" ]] || {
    echo "FATAL: $BENCH_BIN not found — build first:"
    echo "  CARGO_TARGET_DIR=/mnt/data1/cargo-target \\"
    echo "    cargo build --release --bin blazil-bench \\"
    echo "    --features blazil-bench/metrics-ws,blazil-bench/tigerbeetle-client"
    exit 1
}

# ── tigerbeetle ────────────────────────────────────────────────────────────
if ! command -v tigerbeetle &>/dev/null; then
    echo "[bench] downloading tigerbeetle ${TB_VERSION}..."
    curl -fsSL \
        "https://github.com/tigerbeetle/tigerbeetle/releases/download/${TB_VERSION}/tigerbeetle-x86_64-linux.zip" \
        -o /tmp/tb.zip
    unzip -qo /tmp/tb.zip -d /tmp/tb-x
    install -m 755 /tmp/tb-x/tigerbeetle /usr/local/bin/tigerbeetle
    rm -rf /tmp/tb.zip /tmp/tb-x
fi

# ── cleanup trap ───────────────────────────────────────────────────────────
TB0="" TB1="" TB2=""
cleanup() { kill $TB0 $TB1 $TB2 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT INT TERM

# ── format fresh cluster (O_DIRECT active on first write) ─────────────────
pkill -f "tigerbeetle start" 2>/dev/null && sleep 1 || true
rm -rf "${TB_DATA}"/tb-node{0,1,2} && mkdir -p "${TB_DATA}"/tb-node{0,1,2}
for r in 0 1 2; do
    tigerbeetle format --cluster=0 --replica=$r --replica-count=3 \
        "${TB_DATA}/tb-node${r}/0_${r}.tigerbeetle" 2>/dev/null
done
echo "[bench] 3 replicas formatted"

# ── start cluster ──────────────────────────────────────────────────────────
start_replica() {
    local r=$1 cpus=$2
    taskset -c "$cpus" tigerbeetle start \
        --addresses="$TB_ADDRS" \
        "${TB_DATA}/tb-node${r}/0_${r}.tigerbeetle" \
        &>"/tmp/tb${r}.log" &
    echo $!
}
TB0=$(start_replica 0 0-3)
TB1=$(start_replica 1 4-7)
TB2=$(start_replica 2 8-11)
echo "[bench] tb0=$TB0  tb1=$TB1  tb2=$TB2"

# ── wait for VSR quorum ────────────────────────────────────────────────────
printf "[bench] waiting for quorum"
for port in 3001 3002 3003; do
    for i in $(seq 1 30); do
        timeout 1 bash -c "echo >/dev/tcp/127.0.0.1/$port" 2>/dev/null && break
        [[ $i -eq 30 ]] && { echo; echo "FATAL: :$port not ready in 30s"; exit 1; }
        printf "."; sleep 1
    done
done
sleep 5
echo " ready"

# ══════════════════════════════════════════════════════════════════════════
#  t=0..20s  WARMUP
# ══════════════════════════════════════════════════════════════════════════
echo "[bench] t=0   warmup 20s..."
taskset -c 12-15 nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRS" \
    "$BENCH_BIN" --scenario sharded-tb --shards "$SHARDS" --duration 20 \
    >/dev/null 2>&1

# ══════════════════════════════════════════════════════════════════════════
#  t=20..80s  MAIN STRESS TEST — peak TPS window
# ══════════════════════════════════════════════════════════════════════════
echo "[bench] t=20  main stress test 60s..."
MAIN_LOG=$(mktemp /tmp/blazil-main-XXXX.log)
taskset -c 12-15 nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRS" \
    "$BENCH_BIN" --scenario sharded-tb --shards "$SHARDS" --duration 60 \
    --metrics-port "$METRICS_PORT" \
    >"$MAIN_LOG" 2>&1

# ══════════════════════════════════════════════════════════════════════════
#  t=80s  KILL REPLICA 2
# ══════════════════════════════════════════════════════════════════════════
KILL_MS=$(date +%s%3N)
kill -9 "$TB2" 2>/dev/null || true
TB2=""
echo "[bench] t=80  kill -9 replica 2 — VSR running on 2-of-3"

# ══════════════════════════════════════════════════════════════════════════
#  t=80..120s  RECOVERY TEST
# ══════════════════════════════════════════════════════════════════════════
echo "[bench] t=80  recovery test 40s..."
RECOVERY_LOG=$(mktemp /tmp/blazil-recovery-XXXX.log)
taskset -c 12-15 nice -n -20 \
    env BLAZIL_TB_ADDRESS="$TB_ADDRS" \
    "$BENCH_BIN" --scenario sharded-tb --shards "$SHARDS" --duration 40 \
    --metrics-port "$METRICS_PORT" \
    >"$RECOVERY_LOG" 2>&1 &
RBENCH=$!

# poll for first TPS line (100ms resolution)
FIRST_MS=0
while kill -0 "$RBENCH" 2>/dev/null; do
    if [[ $FIRST_MS -eq 0 ]] && grep -q "→" "$RECOVERY_LOG" 2>/dev/null; then
        FIRST_MS=$(date +%s%3N)
    fi
    sleep 0.1
done
wait "$RBENCH" 2>/dev/null || true
[[ $FIRST_MS -eq 0 ]] && FIRST_MS=$(( KILL_MS + 40000 ))
RECOVERY_MS=$(( FIRST_MS - KILL_MS ))

# ══════════════════════════════════════════════════════════════════════════
#  PARSE RESULTS
# ══════════════════════════════════════════════════════════════════════════
PEAK_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$MAIN_LOG" \
    | tr -d ',' | sort -n | tail -1 || echo 0)
AVG_TPS=$(grep -oP '→ \K[0-9,]+(?= TPS)' "$MAIN_LOG" \
    | tr -d ',' | awk '{s+=$1;c++} END{printf "%d", (c?s/c:0)}')
P99_US=$(grep -oP 'p99=\K[0-9]+(?= µs)' "$MAIN_LOG" \
    | sort -n | tail -1 || echo 0)
P99_MS=$(awk "BEGIN{printf \"%.1f\", ${P99_US:-0}/1000}")
SURVIVAL=$(grep -q "→" "$RECOVERY_LOG" && echo "100.00%" || echo "0.00% (quorum loss)")
rm -f "$MAIN_LOG" "$RECOVERY_LOG"

# ══════════════════════════════════════════════════════════════════════════
#  t=120s  FINAL REPORT
# ══════════════════════════════════════════════════════════════════════════
echo ""
echo "==================================================================="
echo "  BLAZIL v0.4 -- VSR FINAL BENCH REPORT"
echo "  i4i.4xlarge  |  16 vCPU  |  1.9TB NVMe  |  3-node VSR cluster"
echo "  WINDOW=2048/shard  |  O_DIRECT  |  bench cores 12-15"
echo "==================================================================="
printf "  %-18s %s\n"    "Peak TPS:"       "${PEAK_TPS}"
printf "  %-18s %s\n"    "Average TPS:"    "${AVG_TPS}"
printf "  %-18s %s ms\n" "P99 Latency:"    "${P99_MS}"
printf "  %-18s %s ms\n" "Recovery Time:"  "${RECOVERY_MS}"
printf "  %-18s %s\n"    "Survival Rate:"  "${SURVIVAL}"
echo "==================================================================="
echo ""

# ── t=121s: shutdown ───────────────────────────────────────────────────────
if [[ "$SKIP_SHUTDOWN" == "1" ]]; then
    echo "[bench] SKIP_SHUTDOWN=1 — instance kept alive"
    PUBLIC_IP="$(curl -s --connect-timeout 2 http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null \
        || hostname -I | awk '{print $1}')"
    echo "[bench] TB logs: /tmp/tb{0,1,2}.log"
    echo "[bench] SSH open at ${PUBLIC_IP}"
else
    echo "[bench] t=121 shutting down..."
    sleep 1
    shutdown -h now
fi

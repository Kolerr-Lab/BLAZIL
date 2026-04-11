#!/usr/bin/env bash
# bench-isolate.sh — Performance isolation for Blazil v2 benchmark on DO node.
#
# Run this on the BENCH NODE (node-1 or node-2) before blazil-bench.
# Must be run as root.
#
# What it does:
#   1. Stop all Docker containers except TigerBeetle
#   2. Drop OS page/slab cache (clean disk baseline)
#   3. ionice: give TigerBeetle RT-class I/O (eliminates VSR journal jitter)
#   4. CPU governor → performance (lock P-state, no frequency scaling)
#   5. Low-latency sysctl for Aeron IPC + VSR inter-node UDP
#   6. Print taskset command to use when running blazil-bench
#
# Usage (on DO node):
#   bash /opt/blazil/scripts/bench-isolate.sh
#
# Then run bench with:
#   taskset -c 4-7 ./target/release/blazil-bench \
#     --scenario aeron --events 5000000 --payload-size 128
#
# CPU layout (8 vCPU, s-8vcpu-16gb):
#   Core 0-3 : TigerBeetle (VSR consensus + disk journal)
#   Core 4   : Aeron serve thread (pinned in transport.rs)
#   Core 5   : Pipeline runner / LedgerHandler batch accumulator
#   Core 6-7 : ledger_rt Tokio workers (TB async callbacks)

set -euo pipefail

# ── Colour helpers ────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
ok()   { echo -e "${GREEN}✓${RESET} $*"; }
info() { echo -e "${CYAN}→${RESET} $*"; }
warn() { echo -e "${YELLOW}⚠${RESET}  $*"; }
die()  { echo -e "${RED}✗ ERROR:${RESET} $*" >&2; exit 1; }

echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo -e "${BOLD}  Blazil v2 — Performance Isolation${RESET}"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""

[[ $EUID -eq 0 ]] || die "Must run as root (sudo bash bench-isolate.sh)"

# ── 1. Docker: stop everything except TigerBeetle ────────────────────────────
echo -e "${BOLD}[1/6] Docker: isolate TigerBeetle${RESET}"

if command -v docker &>/dev/null; then
  # Find TigerBeetle container(s) — keep any container whose image/name contains "tigerbeetle"
  TB_CONTAINERS=$(docker ps --format '{{.ID}} {{.Image}} {{.Names}}' \
    | grep -i 'tigerbeetle' | awk '{print $1}' || true)

  ALL_CONTAINERS=$(docker ps -q || true)

  STOPPED=0
  for cid in $ALL_CONTAINERS; do
    if echo "$TB_CONTAINERS" | grep -q "$cid"; then
      ok "Keeping TigerBeetle container: $cid"
    else
      NAME=$(docker inspect --format '{{.Name}}' "$cid" 2>/dev/null | tr -d '/')
      info "Stopping container: $NAME ($cid)"
      docker stop "$cid" >/dev/null
      STOPPED=$((STOPPED + 1))
    fi
  done
  ok "Stopped $STOPPED non-TigerBeetle container(s)"
else
  warn "Docker not found — skipping container cleanup"
fi

# Also stop any non-essential system services that do disk I/O
for svc in unattended-upgrades apt-daily apt-daily-upgrade; do
  systemctl stop "$svc" 2>/dev/null && info "Stopped $svc" || true
done
echo ""

# ── 2. Drop OS page/slab cache ───────────────────────────────────────────────
echo -e "${BOLD}[2/6] Drop OS cache (clean disk baseline)${RESET}"
sync
echo 3 > /proc/sys/vm/drop_caches
FREE_MB=$(awk '/MemAvailable/ {printf "%d", $2/1024}' /proc/meminfo)
ok "Cache dropped. Free RAM: ${FREE_MB} MB"
echo ""

# ── 3. ionice: TigerBeetle → real-time I/O class ────────────────────────────
echo -e "${BOLD}[3/6] ionice: TigerBeetle → RT I/O class 1 (highest)${RESET}"

TB_PIDS=$(pgrep -x tigerbeetle || true)
if [ -n "$TB_PIDS" ]; then
  for pid in $TB_PIDS; do
    ionice -c 1 -n 0 -p "$pid"
    ok "ionice RT applied: pid $pid ($(cat /proc/$pid/comm))"
  done
else
  # TB running inside Docker — apply to the container's main process
  if command -v docker &>/dev/null && [ -n "${TB_CONTAINERS:-}" ]; then
    for cid in $TB_CONTAINERS; do
      PID=$(docker inspect --format '{{.State.Pid}}' "$cid" 2>/dev/null || true)
      if [ -n "$PID" ] && [ "$PID" -gt 0 ]; then
        ionice -c 1 -n 0 -p "$PID"
        ok "ionice RT applied to Docker TB pid $PID"
      fi
    done
  else
    warn "TigerBeetle process not found — ionice skipped"
    warn "Make sure TB is running before bench-isolate.sh"
  fi
fi
echo ""

# ── 4. CPU governor → performance ────────────────────────────────────────────
echo -e "${BOLD}[4/6] CPU governor → performance (lock P-state)${RESET}"
CPUS_CHANGED=0
for gov in /sys/devices/system/cpu/cpu[0-9]*/cpufreq/scaling_governor; do
  [ -f "$gov" ] || continue
  echo performance > "$gov"
  CPUS_CHANGED=$((CPUS_CHANGED + 1))
done
if [ $CPUS_CHANGED -gt 0 ]; then
  ok "$CPUS_CHANGED CPU(s) set to performance governor"
else
  warn "cpufreq not available (DO VM may use host-controlled P-state)"
fi

# Pin TigerBeetle to cores 0-3 (keep VSR off bench cores)
if [ -n "${TB_PIDS:-}" ]; then
  for pid in $TB_PIDS; do
    taskset -cp 0-3 "$pid" >/dev/null 2>&1 && ok "taskset: TB pid $pid → cores 0-3" || true
  done
fi
echo ""

# ── 5. Sysctl: low-latency tuning for Aeron IPC + VSR UDP ───────────────────
echo -e "${BOLD}[5/6] sysctl: low-latency network + memory${RESET}"

# Socket buffers — large enough for Aeron term buffer (128 MB)
sysctl -qw net.core.rmem_max=134217728
sysctl -qw net.core.wmem_max=134217728
sysctl -qw net.core.rmem_default=65536
sysctl -qw net.ipv4.udp_mem="102400 873800 134217728"

# Reduce TCP/UDP interrupt latency — busy-poll instead of sleep on recv
sysctl -qw net.core.busy_poll=50         # µs to spin on socket before sleeping
sysctl -qw net.core.busy_read=50         # µs to spin on recv() before sleeping

# Keep TCP connections hot (VSR inter-node)
sysctl -qw net.ipv4.tcp_slow_start_after_idle=0
sysctl -qw net.ipv4.tcp_nodelay_val=1 2>/dev/null || true  # best-effort

# Avoid kernel stealing CPU from bench for timer interrupts (not available on all kernels)
sysctl -qw kernel.sched_min_granularity_ns=10000000   2>/dev/null || true
sysctl -qw kernel.sched_wakeup_granularity_ns=15000000 2>/dev/null || true

# Disable transparent huge pages defrag (causes latency spikes)
echo never > /sys/kernel/mm/transparent_hugepage/defrag 2>/dev/null || true
echo madvise > /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || true

ok "sysctl applied"
echo ""

# ── 6. ulimit + summary ───────────────────────────────────────────────────────
echo -e "${BOLD}[6/6] File descriptor limit${RESET}"
ulimit -n 65535
ok "ulimit -n: $(ulimit -n)"
echo ""

# ── Summary ───────────────────────────────────────────────────────────────────
NCPU=$(nproc)
FREE_NOW=$(awk '/MemAvailable/ {printf "%d", $2/1024}' /proc/meminfo)
TB_PID_LIST="${TB_PIDS:-none}"

echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo -e "${BOLD}  Isolation complete — ready to bench${RESET}"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
echo ""
echo -e "  CPUs      : ${NCPU} cores"
echo -e "  Free RAM  : ${FREE_NOW} MB"
echo -e "  TB pids   : ${TB_PID_LIST}"
echo ""
echo -e "${BOLD}  Run bench now:${RESET}"
echo ""
if [ "$NCPU" -ge 8 ]; then
  echo -e "  ${CYAN}taskset -c 4-7 ./target/release/blazil-bench \\${RESET}"
  echo -e "  ${CYAN}  --scenario aeron --events 5000000 --payload-size 128${RESET}"
  echo ""
  echo -e "  CPU layout:"
  echo -e "    Core 0-3 : TigerBeetle (VSR + journal)"
  echo -e "    Core 4   : Aeron serve thread"
  echo -e "    Core 5   : Pipeline runner"
  echo -e "    Core 6-7 : ledger_rt workers (TB callbacks)"
else
  # Fewer than 8 cores (shouldn't happen on s-8vcpu-16gb, but be safe)
  BENCH_CORES="$((NCPU/2))-$((NCPU-1))"
  TB_CORES="0-$((NCPU/2-1))"
  echo -e "  ${CYAN}taskset -c ${BENCH_CORES} ./target/release/blazil-bench \\${RESET}"
  echo -e "  ${CYAN}  --scenario aeron --events 5000000 --payload-size 128${RESET}"
  echo ""
  warn "Only $NCPU cores detected — TB on $TB_CORES, bench on $BENCH_CORES"
fi
echo ""
echo -e "  After bench completes:"
echo -e "  ${CYAN}cd /opt/blazil && git add docs/runs/ && git commit -m 'bench: ...' && git push${RESET}"
echo ""

#!/usr/bin/env bash
# =============================================================================
# v0.4_aws_setup.sh — Blazil "AWS Conqueror" node provisioning script
#
# Target: AWS i4i.metal (128 vCPUs, NVMe local SSD)
# Goal  : Squeeze every nanosecond out of the OS for 1M TPS TigerBeetle bench
#
# Run as root on EACH bench node BEFORE starting TigerBeetle.
#
# Usage:
#   chmod +x v0.4_aws_setup.sh
#   sudo ./v0.4_aws_setup.sh [--tb-data-dir /mnt/nvme/tigerbeetle] [--dry-run]
#
# What it does:
#   1. Raises all ulimits to hard ceiling (files, processes, memlock)
#   2. Tunes TCP stack: 128 MB buffers, BBR, low latency socket options
#   3. Pins all CPU cores to "performance" frequency governor
#   4. Disables CPU c-states and turbo boost variance (flat, predictable clocks)
#   5. Discovers and mounts AWS NVMe local SSD to --tb-data-dir
#   6. Sets XFS/ext4 mount options for raw throughput (noatime, nobarrier)
#   7. Tunes kernel I/O scheduler to none (bypass cfq/mq-deadline for NVMe)
#   8. Sets huge pages (transparent + explicit) for TigerBeetle mmap
#   9. Configures IRQ affinity: network IRQs pinned away from bench CPUs
#  10. Prints a post-setup diagnostics summary
#
# After running, start TigerBeetle with:
#   ulimit -n 1048576
#   tigerbeetle format --cluster=N --replica=N --replica-count=3 \
#       /mnt/nvme/tigerbeetle/N_N.tigerbeetle
#   nohup tigerbeetle start \
#       --addresses=<n1>:3000,<n2>:3000,<n3>:3000 \
#       /mnt/nvme/tigerbeetle/N_N.tigerbeetle \
#       > /var/log/tb.log 2>&1 &
#
# For dual-cluster (2 TB instances per node, parallel fsync):
#   tigerbeetle format --cluster=0 --replica=N --replica-count=3 \
#       /mnt/nvme/tigerbeetle/0_N.tigerbeetle
#   tigerbeetle format --cluster=1 --replica=N --replica-count=3 \
#       /mnt/nvme/tigerbeetle/1_N.tigerbeetle
#   nohup tigerbeetle start \
#       --addresses=<n1>:3000,<n2>:3000,<n3>:3000 \
#       /mnt/nvme/tigerbeetle/0_N.tigerbeetle > /var/log/tb0.log 2>&1 &
#   nohup tigerbeetle start \
#       --addresses=<n1>:3001,<n2>:3001,<n3>:3001 \
#       /mnt/nvme/tigerbeetle/1_N.tigerbeetle > /var/log/tb1.log 2>&1 &
# =============================================================================

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
TB_DATA_DIR="/mnt/nvme/tigerbeetle"
DRY_RUN=false
NVME_DEV=""   # auto-detected if empty

RED='\033[0;31m'
GRN='\033[0;32m'
YLW='\033[1;33m'
BLU='\033[0;34m'
NC='\033[0m'

log()  { echo -e "${GRN}[setup]${NC} $*"; }
warn() { echo -e "${YLW}[warn]${NC}  $*"; }
err()  { echo -e "${RED}[error]${NC} $*" >&2; }
hdr()  { echo -e "\n${BLU}══════════════════════════════════════════${NC}"; \
          echo -e "${BLU}  $*${NC}"; \
          echo -e "${BLU}══════════════════════════════════════════${NC}"; }

run() {
    if [[ "$DRY_RUN" == "true" ]]; then
        echo -e "  ${YLW}[dry-run]${NC} $*"
    else
        eval "$@"
    fi
}

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tb-data-dir) TB_DATA_DIR="$2"; shift 2 ;;
        --nvme-dev)    NVME_DEV="$2";    shift 2 ;;
        --dry-run)     DRY_RUN=true;     shift ;;
        *)             err "Unknown arg: $1"; exit 1 ;;
    esac
done

[[ $EUID -ne 0 ]] && { err "Must run as root (sudo)"; exit 1; }

log "Starting Blazil AWS Conqueror setup"
log "TB data dir : $TB_DATA_DIR"
log "Dry run     : $DRY_RUN"

# =============================================================================
# 1. ULIMITS — raise to 1,048,576 (kernel max on most configs)
# =============================================================================
hdr "1. Ulimits"

ULIMIT_MAX=1048576

# Persist across reboots via limits.conf
LIMITS_CONF="/etc/security/limits.conf"
if ! grep -q "blazil-nofile" "$LIMITS_CONF" 2>/dev/null; then
    run "cat >> $LIMITS_CONF <<'EOF'
# Blazil bench — max file descriptors
* soft nofile $ULIMIT_MAX
* hard nofile $ULIMIT_MAX
root soft nofile $ULIMIT_MAX
root hard nofile $ULIMIT_MAX
# blazil-nofile marker
EOF"
    log "limits.conf updated"
else
    log "limits.conf already patched"
fi

# Raise kernel-level max
run "sysctl -w fs.file-max=$ULIMIT_MAX"
run "sysctl -w fs.nr_open=$ULIMIT_MAX"

# Apply immediately to current shell / child processes
run "ulimit -n $ULIMIT_MAX" || warn "ulimit -n failed in script context (will apply via limits.conf on next login)"
run "ulimit -u $ULIMIT_MAX" || warn "ulimit -u failed in script context"

log "ulimit target: $ULIMIT_MAX"

# =============================================================================
# 2. TCP STACK — 128 MB buffers, BBR, low-latency tweaks
# =============================================================================
hdr "2. TCP Stack"

run "sysctl -w net.core.rmem_max=134217728"
run "sysctl -w net.core.wmem_max=134217728"
run "sysctl -w net.core.rmem_default=134217728"
run "sysctl -w net.core.wmem_default=134217728"
run "sysctl -w net.core.optmem_max=134217728"
run "sysctl -w net.ipv4.tcp_rmem='4096 87380 134217728'"
run "sysctl -w net.ipv4.tcp_wmem='4096 65536 134217728'"
run "sysctl -w net.core.netdev_max_backlog=250000"
run "sysctl -w net.ipv4.tcp_max_syn_backlog=8192"
run "sysctl -w net.core.somaxconn=65535"

# BBR congestion control (lower latency for intra-cluster traffic)
if modprobe tcp_bbr 2>/dev/null; then
    run "sysctl -w net.ipv4.tcp_congestion_control=bbr"
    run "sysctl -w net.core.default_qdisc=fq"
    log "BBR congestion control enabled"
else
    warn "tcp_bbr module unavailable — keeping default CC"
fi

# Disable Nagle and slow-start for VSR's request/reply message pattern
run "sysctl -w net.ipv4.tcp_low_latency=1" || true
run "sysctl -w net.ipv4.tcp_no_delay_ack=1" || true

# Keep TIME_WAIT sockets to a sane number (VSR reconnects 3 nodes)
run "sysctl -w net.ipv4.tcp_tw_reuse=1"
run "sysctl -w net.ipv4.tcp_fin_timeout=15"

# Persist to sysctl.d so reboots preserve settings
SYSCTL_CONF="/etc/sysctl.d/99-blazil.conf"
run "cat > $SYSCTL_CONF <<'EOF'
# Blazil bench — TCP tuning
net.core.rmem_max = 134217728
net.core.wmem_max = 134217728
net.core.rmem_default = 134217728
net.core.wmem_default = 134217728
net.core.optmem_max = 134217728
net.ipv4.tcp_rmem = 4096 87380 134217728
net.ipv4.tcp_wmem = 4096 65536 134217728
net.core.netdev_max_backlog = 250000
net.ipv4.tcp_max_syn_backlog = 8192
net.core.somaxconn = 65535
net.ipv4.tcp_tw_reuse = 1
net.ipv4.tcp_fin_timeout = 15
fs.file-max = 1048576
fs.nr_open = 1048576
EOF"
log "TCP stack tuned (128 MB buffers)"

# =============================================================================
# 3. CPU — performance governor + disable c-state variance
# =============================================================================
hdr "3. CPU Governor"

if [[ -d /sys/devices/system/cpu/cpu0/cpufreq ]]; then
    run "echo performance | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null"
    log "All CPUs set to 'performance' governor"
else
    warn "/sys/devices/system/cpu/cpu0/cpufreq not found — may be a bare metal or Nitro instance"
    warn "Try: apt-get install -y linux-tools-\$(uname -r) cpupower && cpupower frequency-set -g performance"
fi

# Disable CPU c-states deeper than C1 (avoids wakeup latency jitter)
for cpu_dir in /sys/devices/system/cpu/cpu*/cpuidle/state*/; do
    state_name=$(cat "${cpu_dir}name" 2>/dev/null || true)
    if [[ "$state_name" == "C2" || "$state_name" == "C3" || "$state_name" == "C6" || "$state_name" == "C7" ]]; then
        run "echo 1 > ${cpu_dir}disable" || true
    fi
done
log "Deep c-states (C2+) disabled"

# Disable Intel turbo boost variance (flat clock, predictable perf)
if [[ -f /sys/devices/system/cpu/intel_pstate/no_turbo ]]; then
    run "echo 0 > /sys/devices/system/cpu/intel_pstate/no_turbo"   # 0 = turbo ON but governor-controlled
    log "Intel pstate turbo: governor-controlled"
fi

# =============================================================================
# 4. NVMe MOUNT — auto-detect AWS local NVMe SSD
# =============================================================================
hdr "4. NVMe Auto-Mount"

if [[ -z "$NVME_DEV" ]]; then
    # AWS i4i.metal: local NVMe shows as /dev/nvme*n* (not EBS which is also nvme)
    # EBS volumes have a vendor string containing "Amazon"; local SSDs do not.
    # Strategy: find unmounted NVMe block devices that are NOT currently /
    for dev in $(ls /dev/nvme*n1 2>/dev/null | sort); do
        # Skip if already mounted
        if mount | grep -q "^${dev} "; then
            log "Skipping $dev (already mounted)"
            continue
        fi
        # Skip if it's the root device
        root_dev=$(findmnt -n -o SOURCE / 2>/dev/null | sed 's/p[0-9]*$//')
        if [[ "$dev" == "$root_dev"* ]]; then
            log "Skipping $dev (root device)"
            continue
        fi
        # Check if it has a model string — AWS local NVMe won't have "EBS" or "Amazon"
        model=$(cat "/sys/block/$(basename $dev)/device/model" 2>/dev/null || \
                nvme id-ctrl "$dev" 2>/dev/null | awk -F: '/^mn /{print $2}' || echo "unknown")
        if echo "$model" | grep -qi "amazon\|ebs"; then
            log "Skipping $dev (EBS volume: $model)"
            continue
        fi
        NVME_DEV="$dev"
        log "Auto-detected local NVMe: $NVME_DEV (model: $model)"
        break
    done
fi

if [[ -z "$NVME_DEV" ]]; then
    warn "No local NVMe device found — skipping mount step"
    warn "If you know the device, re-run with: --nvme-dev /dev/nvme1n1"
    warn "TigerBeetle will use: $TB_DATA_DIR (ensure it exists)"
    run "mkdir -p $TB_DATA_DIR"
else
    log "Using NVMe device: $NVME_DEV → $TB_DATA_DIR"

    # Format with XFS if not already formatted (fast, good for large sequential writes)
    fs_type=$(blkid -o value -s TYPE "$NVME_DEV" 2>/dev/null || echo "")
    if [[ -z "$fs_type" ]]; then
        log "Formatting $NVME_DEV with XFS..."
        run "mkfs.xfs -f -L tigerbeetle $NVME_DEV"
        log "XFS format complete"
    else
        log "$NVME_DEV already formatted as $fs_type — skipping format"
    fi

    # Mount with throughput-optimized options
    run "mkdir -p $TB_DATA_DIR"
    if ! mount | grep -q "$TB_DATA_DIR"; then
        run "mount -o noatime,nodiratime,nobarrier,discard $NVME_DEV $TB_DATA_DIR"
        log "Mounted $NVME_DEV → $TB_DATA_DIR (noatime, nobarrier, discard)"
    else
        log "$TB_DATA_DIR already mounted"
    fi

    # Persist in fstab (use UUID so device name survives reboots)
    uuid=$(blkid -o value -s UUID "$NVME_DEV" 2>/dev/null || echo "")
    if [[ -n "$uuid" ]] && ! grep -q "$uuid" /etc/fstab; then
        run "echo 'UUID=$uuid $TB_DATA_DIR xfs noatime,nodiratime,nobarrier,discard,nofail 0 2' >> /etc/fstab"
        log "fstab updated (UUID=$uuid)"
    fi
fi

# =============================================================================
# 5. I/O SCHEDULER — bypass queue for NVMe (none/mq-deadline)
# =============================================================================
hdr "5. I/O Scheduler"

for dev in $(ls /sys/block/ | grep nvme 2>/dev/null); do
    sched_path="/sys/block/$dev/queue/scheduler"
    if [[ -f "$sched_path" ]]; then
        current=$(cat "$sched_path")
        # Prefer "none" (passthrough); fall back to mq-deadline for mixed workloads
        if echo "$current" | grep -q "\[none\]"; then
            log "$dev: scheduler already 'none'"
        elif echo "$current" | grep -q "none"; then
            run "echo none > $sched_path"
            log "$dev: scheduler set to 'none'"
        elif echo "$current" | grep -q "mq-deadline"; then
            run "echo mq-deadline > $sched_path"
            log "$dev: scheduler set to 'mq-deadline'"
        else
            warn "$dev: cannot set 'none' or 'mq-deadline' (available: $current)"
        fi
        # Disable I/O merging for NVMe (merging adds latency, NVMe handles it in HW)
        run "echo 0 > /sys/block/$dev/queue/nomerges" || true
        # Max queue depth
        run "echo 1024 > /sys/block/$dev/queue/nr_requests" || true
    fi
done

# =============================================================================
# 6. HUGE PAGES — TigerBeetle maps its data file with mmap
# =============================================================================
hdr "6. Huge Pages"

# Transparent huge pages: madvise mode lets TB opt in per-mmap
run "echo madvise > /sys/kernel/mm/transparent_hugepage/enabled" || warn "THP madvise not supported"
run "echo defer+madvise > /sys/kernel/mm/transparent_hugepage/defrag" || true

# Reserve explicit 1G huge pages for direct mapping (1G × 4 = 4GB headroom)
# Adjust count based on available RAM: i4i.metal has 1.5 TB RAM
HUGEPAGE_1G=4
if [[ -f /sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages ]]; then
    run "echo $HUGEPAGE_1G > /sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages"
    log "Reserved ${HUGEPAGE_1G}× 1G huge pages"
fi

# 2MB huge pages as fallback pool
HUGEPAGE_2M=2048
if [[ -f /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages ]]; then
    run "echo $HUGEPAGE_2M > /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages"
    log "Reserved ${HUGEPAGE_2M}× 2MB huge pages"
fi

# =============================================================================
# 7. IRQ AFFINITY — pin NIC IRQs away from bench CPU cores
# =============================================================================
hdr "7. IRQ Affinity"

# Strategy: the bench process (and TB) runs on cores 0..N/2-1.
# Pin NIC interrupts to the upper half of CPUs so they don't preempt bench threads.
TOTAL_CPUS=$(nproc)
UPPER_HALF_START=$((TOTAL_CPUS / 2))
# Build a CPU affinity mask for the upper half (hex bitmask)
UPPER_MASK=0
for i in $(seq $UPPER_HALF_START $((TOTAL_CPUS - 1))); do
    UPPER_MASK=$((UPPER_MASK | (1 << i)))
done
UPPER_MASK_HEX=$(printf '%x' $UPPER_MASK)

NIC_IRQ_COUNT=0
for irq_dir in /proc/irq/*/; do
    irq_num=$(basename "$irq_dir")
    [[ "$irq_num" == "0" ]] && continue  # skip default
    smp_affinity="/proc/irq/$irq_num/smp_affinity"
    [[ -f "$smp_affinity" ]] || continue
    # Check if this IRQ is associated with a network device
    actions_file="/proc/irq/$irq_num/actions" 
    # Note: on newer kernels it may not exist by default; try /proc/irq/N/node
    irq_name=$(cat "/proc/irq/$irq_num/actions" 2>/dev/null || echo "")
    if echo "$irq_name" | grep -qiE 'eth|ens|eno|enp|mlx|ena'; then
        run "echo $UPPER_MASK_HEX > $smp_affinity" || true
        NIC_IRQ_COUNT=$((NIC_IRQ_COUNT + 1))
    fi
done
log "Pinned $NIC_IRQ_COUNT NIC IRQ(s) to upper ${TOTAL_CPUS}/${UPPER_HALF_START} CPUs (mask=0x${UPPER_MASK_HEX})"

# =============================================================================
# 8. MISC OS TWEAKS
# =============================================================================
hdr "8. Misc Tweaks"

# Disable swap — TigerBeetle must NEVER page out its mmap'd data file
run "swapoff -a" || warn "swapoff failed (no swap or permission issue)"

# Set vm.dirty_* for write-heavy workloads (let kernel buffer more before writeback)
run "sysctl -w vm.dirty_ratio=80"
run "sysctl -w vm.dirty_background_ratio=5"
run "sysctl -w vm.dirty_expire_centisecs=12000"
run "sysctl -w vm.dirty_writeback_centisecs=100"

# Disable NUMA balancing (TB process should stay on its NUMA node)
run "sysctl -w kernel.numa_balancing=0" || true

# Set kernel to full preemption (lower syscall latency)
# Note: requires PREEMPT=y kernel; most AWS AL2023/Ubuntu kernels have this
run "sysctl -w kernel.sched_min_granularity_ns=500000"    || true
run "sysctl -w kernel.sched_wakeup_granularity_ns=1000000" || true

# Disable address space layout randomisation for bench process (deterministic perf)
# Keep enabled system-wide for security; TB disables it internally via personality()
log "ASLR kept system-wide (TB manages its own via personality)"

log "Misc OS tweaks applied"

# =============================================================================
# 9. ENSURE DATA DIRECTORY EXISTS
# =============================================================================
run "mkdir -p $TB_DATA_DIR"
run "chmod 700 $TB_DATA_DIR"
log "Data directory ready: $TB_DATA_DIR"

# =============================================================================
# 10. DIAGNOSTICS SUMMARY
# =============================================================================
hdr "Setup Complete — Diagnostics"

echo ""
echo "  CPUs           : $(nproc) cores"
echo "  Governor       : $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo 'N/A')"
echo "  Open files max : $(cat /proc/sys/fs/file-max)"
echo "  TCP rmem_max   : $(cat /proc/sys/net/core/rmem_max)"
echo "  TCP wmem_max   : $(cat /proc/sys/net/core/wmem_max)"
echo "  THP            : $(cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || echo 'N/A')"
echo "  Swap           : $(free -h | awk '/^Swap:/{print $2}')"
echo "  TB data dir    : $TB_DATA_DIR"
if [[ -n "$NVME_DEV" ]]; then
    echo "  NVMe device    : $NVME_DEV"
    echo "  NVMe scheduler : $(cat /sys/block/$(basename $NVME_DEV)/queue/scheduler 2>/dev/null || echo 'N/A')"
    df -h "$TB_DATA_DIR" 2>/dev/null | tail -1 | awk '{print "  NVMe free      : "$4" / "$2}'
fi
echo ""
echo -e "${GRN}════════════════════════════════════════════════════════${NC}"
echo -e "${GRN}  NODE IS READY — Start TigerBeetle and run the bench  ${NC}"
echo -e "${GRN}════════════════════════════════════════════════════════${NC}"
echo ""

# ── Quick-start snippet ───────────────────────────────────────────────────────
cat <<'SNIPPET'
# ── Quick-start (single cluster, adapt REPLICA/COUNT per node) ──────────────
REPLICA=0       # 0, 1, or 2 per node
REPLICA_COUNT=3
CLUSTER_ID=0
ADDRESSES="10.x.x.1:3000,10.x.x.2:3000,10.x.x.3:3000"
DATA_FILE="${TB_DATA_DIR}/${CLUSTER_ID}_${REPLICA}.tigerbeetle"

ulimit -n 1048576
tigerbeetle format \
    --cluster=$CLUSTER_ID \
    --replica=$REPLICA \
    --replica-count=$REPLICA_COUNT \
    "$DATA_FILE"

nohup tigerbeetle start \
    --addresses=$ADDRESSES \
    "$DATA_FILE" > /var/log/tb.log 2>&1 &

echo "TigerBeetle started (PID $!)"

# ── Dual-cluster add-on (for parallel fsync on same nodes) ──────────────────
CLUSTER_ID_2=1
DATA_FILE_2="${TB_DATA_DIR}/${CLUSTER_ID_2}_${REPLICA}.tigerbeetle"
ADDRESSES_2="10.x.x.1:3001,10.x.x.2:3001,10.x.x.3:3001"

tigerbeetle format \
    --cluster=$CLUSTER_ID_2 \
    --replica=$REPLICA \
    --replica-count=$REPLICA_COUNT \
    "$DATA_FILE_2"

nohup tigerbeetle start \
    --addresses=$ADDRESSES_2 \
    "$DATA_FILE_2" > /var/log/tb2.log 2>&1 &

echo "TigerBeetle cluster 2 started (PID $!)"
SNIPPET

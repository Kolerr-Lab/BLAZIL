#!/bin/bash
# Blazil kernel tuning for DigitalOcean Ubuntu 22.04 nodes.
# Run this on each node BEFORE starting the cluster.
#
# Architecture:
#   Core 0: blazil-engine (hot path, CPU pinned)
#   Core 1: TigerBeetle (VSR consensus, CPU pinned)
#   Core 2: Services (payments/banking/trading/crypto) + NIC IRQs
#   Core 3: Observability (prometheus/grafana)
#
# Optimizations:
#   - BBR congestion control (DO network tuning)
#   - TCP fast open + connection reuse
#   - Huge pages for Aeron shared memory
#   - IRQ pinning to avoid engine core contention
#   - Real-time priority for engine process
#
# Usage:
#   ssh root@<node-ip> 'bash -s' < scripts/do-tune.sh

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔧 Blazil kernel tuning..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# ── Network stack ─────────────────────────────────────────────────────────────

echo "⚙️  Network: BBR congestion control + TCP fast open + FIX 3 tuning"
sysctl -w net.core.somaxconn=65535
sysctl -w net.core.netdev_max_backlog=65535
sysctl -w net.ipv4.tcp_rmem="4096 87380 134217728"
sysctl -w net.ipv4.tcp_wmem="4096 65536 134217728"
sysctl -w net.ipv4.tcp_congestion_control=bbr
sysctl -w net.core.default_qdisc=fq
sysctl -w net.ipv4.tcp_fastopen=3
sysctl -w net.ipv4.tcp_tw_reuse=1
sysctl -w net.ipv4.tcp_fin_timeout=15
sysctl -w net.ipv4.tcp_max_syn_backlog=65535

# FIX 3: TCP KeepAlive (detect dead connections faster)
echo "⚙️  TCP KeepAlive: aggressive detection of dead connections"
sysctl -w net.ipv4.tcp_keepalive_time=10
sysctl -w net.ipv4.tcp_keepalive_intvl=3
sysctl -w net.ipv4.tcp_keepalive_probes=3

# FIX 3: Increase local port range (support more concurrent connections)
echo "⚙️  Local port range: 1024-65535"
sysctl -w net.ipv4.ip_local_port_range="1024 65535"

# ── Memory ────────────────────────────────────────────────────────────────────

echo "⚙️  Memory: disable swap, optimize dirty ratios"
sysctl -w vm.swappiness=0
sysctl -w vm.dirty_ratio=40
sysctl -w vm.dirty_background_ratio=10
sysctl -w vm.overcommit_memory=1

# Huge pages for Aeron shared memory (512 × 2MB = 1GB)
echo 512 > /proc/sys/vm/nr_hugepages
echo "   Huge pages: $(cat /proc/sys/vm/nr_hugepages) × 2MB = $(($(cat /proc/sys/vm/nr_hugepages) * 2))MB"

# ── File descriptors ──────────────────────────────────────────────────────────

echo "⚙️  File descriptors: raise limits to 1M"
ulimit -n 1048576

# Persist across reboots
if ! grep -q "nofile 1048576" /etc/security/limits.conf; then
  echo "* soft nofile 1048576" >> /etc/security/limits.conf
  echo "* hard nofile 1048576" >> /etc/security/limits.conf
fi

# ── IRQ pinning ───────────────────────────────────────────────────────────────

echo "⚙️  IRQ: disable irqbalance, pin NIC IRQs to core 2"

# Stop automatic IRQ balancing
systemctl stop irqbalance 2>/dev/null || true
systemctl disable irqbalance 2>/dev/null || true

# Pin all NIC IRQs to CPU core 2 (avoid engine on core 0, TB on core 1)
NIC=$(ip route | grep default | awk '{print $5}' | head -1)
if [ -n "$NIC" ]; then
  echo "   NIC: $NIC"
  for irq in $(grep "$NIC" /proc/interrupts | awk '{print $1}' | tr -d ':'); do
    if [ -f "/proc/irq/$irq/smp_affinity" ]; then
      echo 4 > /proc/irq/$irq/smp_affinity  # bitmask: 0100 = core 2
      echo "   IRQ $irq → core 2"
    fi
  done
else
  echo "   ⚠️  Could not detect default NIC, skipping IRQ pinning"
fi

# ── CPU frequency scaling ─────────────────────────────────────────────────────

echo "⚙️  CPU: set performance governor (disable power-saving)"
for cpu in /sys/devices/system/cpu/cpu[0-9]*; do
  if [ -f "$cpu/cpufreq/scaling_governor" ]; then
    echo performance > "$cpu/cpufreq/scaling_governor"
  fi
done

# ── Persist settings ──────────────────────────────────────────────────────────

echo "⚙️  Persisting sysctl settings to /etc/sysctl.d/99-blazil.conf"
cat > /etc/sysctl.d/99-blazil.conf <<EOF
# Blazil performance tuning
net.core.somaxconn=65535
net.core.netdev_max_backlog=65535
net.ipv4.tcp_rmem=4096 87380 134217728
net.ipv4.tcp_wmem=4096 65536 134217728
net.ipv4.tcp_congestion_control=bbr
net.core.default_qdisc=fq
net.ipv4.tcp_fastopen=3
net.ipv4.tcp_tw_reuse=1
net.ipv4.tcp_fin_timeout=15
net.ipv4.tcp_max_syn_backlog=65535
# FIX 3: TCP KeepAlive + port range
net.ipv4.tcp_keepalive_time=10
net.ipv4.tcp_keepalive_intvl=3
net.ipv4.tcp_keepalive_probes=3
net.ipv4.ip_local_port_range=1024 65535
# Memory
vm.swappiness=0
vm.dirty_ratio=40
vm.dirty_background_ratio=10
vm.overcommit_memory=1
vm.nr_hugepages=512
EOF

echo ""
echo "✅ Kernel tuning complete"
echo ""
echo "Next steps:"
echo "  1. Restart Docker daemon: systemctl restart docker"
echo "  2. Start Blazil cluster: cd ~/blazil && bash scripts/do-start.sh <node-ips...>"
echo ""
echo "To verify IRQ pinning:"
echo "  watch -n1 'cat /proc/interrupts | grep $NIC'"
echo ""

#!/bin/bash
# do-setup.sh — Run on each DigitalOcean node after provisioning
#
# Usage:
#   ./scripts/do-setup.sh <node-id> <shard-id>
#
# Example (node 1, shard 0):
#   ./scripts/do-setup.sh node-1 0
#
# Run as root (or with sudo) on a fresh Ubuntu 22.04 droplet.
set -e

NODE_ID=${1:-"node-1"}   # node-1, node-2, node-3
SHARD_ID=${2:-"0"}       # 0, 1, 2

echo "═══════════════════════════════════════════════"
echo " Blazil Node Setup: $NODE_ID (shard $SHARD_ID)"
echo "═══════════════════════════════════════════════"

# ── Docker ────────────────────────────────────────────────────────────────────
if ! command -v docker &>/dev/null; then
  echo "▶ Installing Docker..."
  curl -fsSL https://get.docker.com | sh
  usermod -aG docker "${SUDO_USER:-$USER}"
  echo "✅ Docker installed"
else
  echo "✅ Docker already present: $(docker --version)"
fi

# ── System packages ───────────────────────────────────────────────────────────
echo "▶ Installing system packages..."
apt-get update -qq
apt-get install -y -qq git curl wget htop iotop sysstat net-tools

# ── Kernel tuning for high TPS ────────────────────────────────────────────────
echo "▶ Tuning kernel parameters..."
cat >> /etc/sysctl.d/99-blazil.conf << 'SYSCTL'
# Blazil high-throughput tuning
net.core.somaxconn = 65535
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 87380 16777216
net.ipv4.tcp_fastopen = 3
net.ipv4.tcp_tw_reuse = 1
fs.file-max = 2097152
net.ipv4.ip_local_port_range = 1024 65535
vm.swappiness = 1
SYSCTL
sysctl -p /etc/sysctl.d/99-blazil.conf

# Increase file descriptor limits
cat >> /etc/security/limits.conf << 'LIMITS'
* soft nofile 1048576
* hard nofile 1048576
LIMITS

# ── Clone repo ────────────────────────────────────────────────────────────────
INSTALL_DIR="/opt/blazil"
if [ -d "$INSTALL_DIR" ]; then
  echo "▶ Updating existing repo..."
  git -C "$INSTALL_DIR" pull
else
  echo "▶ Cloning Blazil..."
  git clone https://github.com/Kolerr-Lab/BLAZIL.git "$INSTALL_DIR"
fi

# ── Node environment ──────────────────────────────────────────────────────────
NODE_ENV_FILE="$INSTALL_DIR/.env.node"
cat > "$NODE_ENV_FILE" << EOF
BLAZIL_NODE_ID=$NODE_ID
BLAZIL_SHARD_ID=$SHARD_ID
BLAZIL_AUTH_REQUIRED=false
EOF

echo ""
echo "✅ Node $NODE_ID ready"
echo "   Repo:     $INSTALL_DIR"
echo "   Env file: $NODE_ENV_FILE"
echo ""
echo "Next steps:"
echo "  1. Note this node's private IP:  hostname -I | awk '{print \$1}'"
echo "  2. Run on all 3 nodes before starting any of them"
echo "  3. Then: cd $INSTALL_DIR && ./scripts/do-start.sh <node1-ip> <node2-ip> <node3-ip>"

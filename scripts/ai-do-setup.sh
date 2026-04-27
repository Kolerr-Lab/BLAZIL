#!/bin/bash
# ai-do-setup.sh — Setup Blazil AI inference on DigitalOcean droplet
#
# Usage:
#   ./scripts/ai-do-setup.sh [node-id]
#
# Example:
#   ./scripts/ai-do-setup.sh ai-node-1
#
# Hardware: DO Premium AMD NVMe (s-4vcpu-8gb-amd)
#   - 4 vCPU @ 2.0 GHz
#   - 8 GB RAM
#   - 160 GB NVMe SSD
#   - $84/month ($0.125/hour)
#
# Run as root (or with sudo) on fresh Ubuntu 22.04 droplet.
set -e

NODE_ID=${1:-"ai-node-1"}

echo "═══════════════════════════════════════════════"
echo " Blazil AI Node Setup: $NODE_ID"
echo "═══════════════════════════════════════════════"

# ── System packages ───────────────────────────────────────────────────────────
echo "▶ Installing system packages..."
apt-get update -qq
apt-get install -y -qq \
  curl wget git htop iotop sysstat net-tools \
  build-essential pkg-config libssl-dev clang

# ── Rust toolchain ────────────────────────────────────────────────────────────
if ! command -v rustc &>/dev/null; then
  echo "▶ Installing Rust 1.88.0..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.88.0
  source "$HOME/.cargo/env"
  echo "✅ Rust installed: $(rustc --version)"
else
  echo "✅ Rust already present: $(rustc --version)"
fi

# Ensure Rust is in PATH for subsequent commands
export PATH="$HOME/.cargo/bin:$PATH"

# ── Kernel tuning ─────────────────────────────────────────────────────────────
echo "▶ Tuning kernel parameters..."
cat >> /etc/sysctl.d/99-blazil-ai.conf << 'SYSCTL'
# Blazil AI inference tuning
net.core.somaxconn = 65535
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 87380 16777216
fs.file-max = 2097152
vm.swappiness = 1
SYSCTL
sysctl -p /etc/sysctl.d/99-blazil-ai.conf

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
  git clone --recurse-submodules https://github.com/Kolerr-Lab/BLAZIL.git "$INSTALL_DIR"
fi

cd "$INSTALL_DIR"

# ── Build AI stack ────────────────────────────────────────────────────────────
echo "▶ Building Blazil AI inference stack (release mode)..."
echo "   This will take 10-15 minutes on 4 vCPU..."

# Build inference library + dataloader
cargo build --release -p blazil-inference

# Build ml-bench tool
cargo build --release -p ml-bench

echo "✅ Build complete"

# ── Download ONNX models ──────────────────────────────────────────────────────
MODEL_DIR="$INSTALL_DIR/models"
mkdir -p "$MODEL_DIR"

echo "▶ Downloading SqueezeNet 1.1 model (~5 MB)..."
if [ ! -f "$MODEL_DIR/squeezenet1.1.onnx" ]; then
  curl -L -o "$MODEL_DIR/squeezenet1.1.onnx" \
    https://github.com/onnx/models/raw/main/validated/vision/classification/squeezenet/model/squeezenet1.1-7.onnx
  echo "✅ SqueezeNet 1.1 downloaded"
else
  echo "✅ SqueezeNet 1.1 already exists"
fi

echo "▶ Downloading ResNet-50 model (~100 MB)..."
if [ ! -f "$MODEL_DIR/resnet50.onnx" ]; then
  curl -L -o "$MODEL_DIR/resnet50.onnx" \
    https://github.com/onnx/models/raw/main/validated/vision/classification/resnet/model/resnet50-v1-7.onnx
  echo "✅ ResNet-50 downloaded"
else
  echo "✅ ResNet-50 already exists"
fi

# ── Generate synthetic dataset ────────────────────────────────────────────────
DATA_DIR="/data/blazil-ai"
mkdir -p "$DATA_DIR"

echo "▶ Generating synthetic dataset (1M samples, ~2 GB)..."
# ml-bench will generate synthetic images on first run
# We just create the directory structure here
mkdir -p "$DATA_DIR/synthetic"
echo "✅ Dataset directory ready: $DATA_DIR"

# ── Create benchmark script ───────────────────────────────────────────────────
cat > "$INSTALL_DIR/run-ai-bench.sh" << 'BENCHSCRIPT'
#!/bin/bash
# Quick benchmark launcher
set -e

BLAZIL_ROOT="/opt/blazil"
MODEL_DIR="$BLAZIL_ROOT/models"
DATA_DIR="/data/blazil-ai"
RESULTS_DIR="$BLAZIL_ROOT/docs/benchmark-screenshots"

mkdir -p "$RESULTS_DIR"
TIMESTAMP=$(date +%Y-%m-%d_%H-%M-%S)
LOG_FILE="$RESULTS_DIR/ai-bench-$TIMESTAMP.log"

exec > >(tee -a "$LOG_FILE") 2>&1

echo "╔══════════════════════════════════════════════════════════╗"
echo "║   BLAZIL AI INFERENCE — DO BENCHMARK                    ║"
echo "║   SqueezeNet 1.1 · 1M samples · synthetic dataset       ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo ""
echo "  Model     : SqueezeNet 1.1 (~5 MB)"
echo "  Samples   : 1,000,000"
echo "  Batch size: 64"
echo "  Workers   : 4"
echo "  Log       : $LOG_FILE"
echo ""

# Dataloader benchmark
echo "═══ Phase 1: Dataloader Throughput ═══"
"$BLAZIL_ROOT/target/release/ml-bench" \
  --mode dataloader \
  --dataset synthetic \
  --path "$DATA_DIR/synthetic" \
  --batch-size 256 \
  --num-workers 4 \
  --duration 60

echo ""
echo "═══ Phase 2: Inference E2E (SqueezeNet) ═══"
"$BLAZIL_ROOT/target/release/ml-bench" \
  --mode inference \
  --model "$MODEL_DIR/squeezenet1.1.onnx" \
  --dataset synthetic \
  --path "$DATA_DIR/synthetic" \
  --batch-size 64 \
  --inference-workers 4 \
  --duration 120

echo ""
echo "✅ Benchmark complete!"
echo "   Log saved to: $LOG_FILE"
BENCHSCRIPT

chmod +x "$INSTALL_DIR/run-ai-bench.sh"

# ── System info ───────────────────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════"
echo " Setup Complete!"
echo "═══════════════════════════════════════════════"
echo ""
echo "  Node ID      : $NODE_ID"
echo "  Install dir  : $INSTALL_DIR"
echo "  Models       : $MODEL_DIR"
echo "  Data         : $DATA_DIR"
echo ""
echo "  CPU info:"
lscpu | grep -E "^Model name|^CPU\(s\):|^Thread|^Core"
echo ""
echo "  Memory:"
free -h | grep -E "^Mem:"
echo ""
echo "  Disk:"
df -h "$INSTALL_DIR" "$DATA_DIR"
echo ""
echo "═══════════════════════════════════════════════"
echo " Next Steps:"
echo "═══════════════════════════════════════════════"
echo ""
echo "  1. Run benchmark:"
echo "     cd $INSTALL_DIR"
echo "     ./run-ai-bench.sh"
echo ""
echo "  2. Monitor metrics (optional):"
echo "     htop           # CPU/memory"
echo "     iostat -x 1    # Disk I/O"
echo ""
echo "  3. View results:"
echo "     cat $INSTALL_DIR/docs/benchmark-screenshots/ai-bench-*.log"
echo ""

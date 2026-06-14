#!/usr/bin/env bash
# Provision an AWS i4i.4xlarge node for ClarkenAI CPU-only inference + Blazil co-location.
#
# This script is intended to run on the target cloud host with the repo already present
# or after cloning it there. It applies deterministic CPU/memory tuning, installs build
# dependencies, and optionally builds the inference binaries used by the benchmark harness.

set -euo pipefail

REPO_ROOT="${PWD}"
MODEL_CACHE_DIR="/opt/clarkenai/models"
AERON_DIR="/dev/shm/aeron-inference-hybrid"
BUILD_BINARIES=true
RUST_TOOLCHAIN="1.88.0"
INSTALL_HF_CLI=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-root)
      REPO_ROOT="$2"; shift 2 ;;
    --model-cache-dir)
      MODEL_CACHE_DIR="$2"; shift 2 ;;
    --aeron-dir)
      AERON_DIR="$2"; shift 2 ;;
    --skip-build)
      BUILD_BINARIES=false; shift 1 ;;
    --skip-hf-cli)
      INSTALL_HF_CLI=false; shift 1 ;;
    --rust-toolchain)
      RUST_TOOLCHAIN="$2"; shift 2 ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 1 ;;
  esac
done

if [[ $EUID -ne 0 ]]; then
  echo "Must run as root (sudo)." >&2
  exit 1
fi

if [[ ! -d "$REPO_ROOT" ]]; then
  echo "Repository root not found: $REPO_ROOT" >&2
  exit 1
fi

log() {
  printf '[setup] %s\n' "$*"
}

log "Installing system packages..."
apt-get update -qq
apt-get install -y -qq \
  build-essential \
  ca-certificates \
  clang \
  curl \
  git \
  htop \
  iotop \
  jq \
  libssl-dev \
  numactl \
  pkg-config \
  python3 \
  python3-pip \
  sysstat

if ! command -v rustc >/dev/null 2>&1; then
  log "Installing Rust ${RUST_TOOLCHAIN}..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain "$RUST_TOOLCHAIN"
fi

export PATH="/root/.local/bin:/usr/local/bin:$HOME/.cargo/bin:$PATH"

if [[ "$INSTALL_HF_CLI" == true ]] && ! command -v hf >/dev/null 2>&1 && ! command -v huggingface-cli >/dev/null 2>&1; then
  log "Installing Hugging Face CLI..."
  python3 -m pip install --break-system-packages -U "huggingface_hub[cli]"
fi

log "Applying kernel and process limits..."
cat >/etc/sysctl.d/99-clarkenai.conf <<EOF
fs.file-max = 1048576
fs.nr_open = 1048576
vm.swappiness = 1
vm.max_map_count = 1048576
vm.zone_reclaim_mode = 0
net.core.somaxconn = 65535
net.core.rmem_max = 134217728
net.core.wmem_max = 134217728
net.core.netdev_max_backlog = 250000
net.ipv4.tcp_rmem = 4096 87380 134217728
net.ipv4.tcp_wmem = 4096 65536 134217728
EOF
sysctl --system >/dev/null

cat >/etc/security/limits.d/99-clarkenai.conf <<EOF
* soft nofile 1048576
* hard nofile 1048576
root soft nofile 1048576
root hard nofile 1048576
EOF

log "Setting CPU governor to performance when available..."
if [[ -d /sys/devices/system/cpu/cpu0/cpufreq ]]; then
  echo performance | tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor >/dev/null
fi

log "Configuring transparent huge pages..."
if [[ -f /sys/kernel/mm/transparent_hugepage/enabled ]]; then
  echo always >/sys/kernel/mm/transparent_hugepage/enabled
fi

log "Preparing runtime directories..."
mkdir -p "$MODEL_CACHE_DIR"
mkdir -p "$(dirname "$AERON_DIR")"
if [[ "$AERON_DIR" == /dev/shm/* ]]; then
  mkdir -p "$AERON_DIR"
else
  mkdir -p "$AERON_DIR"
fi

log "Repository root: $REPO_ROOT"
log "Model cache dir: $MODEL_CACHE_DIR"
log "Aeron dir: $AERON_DIR"

if [[ "$BUILD_BINARIES" == true ]]; then
  log "Building release inference binaries..."
  cd "$REPO_ROOT"
  cargo build --release -p blazil-inference-service -p test-inference
fi

log "Setup complete."
log "Next step: run scripts/get-clarkenai-70b-model.sh to fetch a 70B/72B GGUF, export CLARKENAI_MODEL_PATH, then run scripts/clarkenai-70b-bench.sh."

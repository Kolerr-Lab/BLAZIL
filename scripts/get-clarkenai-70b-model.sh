#!/usr/bin/env bash

set -euo pipefail

if [[ "$(uname -s)" == "Linux" ]]; then
  DEFAULT_DIR="/opt/clarkenai/models"
else
  DEFAULT_DIR="/Users/rickyanhnguyen/models"
fi

MODEL_DIR="${CLARKENAI_MODEL_DIR:-$DEFAULT_DIR}"
PROFILE="qwen72-q4km"
REQUIRED_GB="0"

usage() {
  cat <<'EOF'
Usage:
  scripts/get-clarkenai-70b-model.sh [--profile NAME] [--model-dir PATH]

Profiles:
  qwen72-q4km    Qwen2.5-72B-Instruct-Q4_K_M.gguf   (~47.4 GB, default)
  qwen72-q5km    Qwen2.5-72B-Instruct-Q5_K_M.gguf   (~54.5 GB)
  qwen72-q3xl    Qwen2.5-72B-Instruct-Q3_K_XL.gguf  (~40.6 GB)
  deepseek32-q4km DeepSeek-R1-Distill-Qwen-32B-Q4_K_M.gguf (~19.9 GB)
  deepseek70-q4km DeepSeek-R1-Distill-Llama-70B-Q4_K_M.gguf (~42.5 GB)
  llama70-q4km   Meta-Llama-3.1-70B-Instruct-Q4_K_M.gguf (~42.5 GB, likely gated)

Examples:
  scripts/get-clarkenai-70b-model.sh
  scripts/get-clarkenai-70b-model.sh --profile qwen72-q5km --model-dir /opt/clarkenai/models
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      PROFILE="$2"; shift 2 ;;
    --model-dir)
      MODEL_DIR="$2"; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown arg: $1" >&2
      usage >&2
      exit 1 ;;
  esac
done

case "$PROFILE" in
  qwen72-q4km)
    REPO="bartowski/Qwen2.5-72B-Instruct-GGUF"
    INCLUDE="Qwen2.5-72B-Instruct-Q4_K_M.gguf"
    REQUIRED_GB="48"
    ;;
  qwen72-q5km)
    REPO="bartowski/Qwen2.5-72B-Instruct-GGUF"
    INCLUDE="Qwen2.5-72B-Instruct-Q5_K_M.gguf"
    REQUIRED_GB="55"
    ;;
  qwen72-q3xl)
    REPO="bartowski/Qwen2.5-72B-Instruct-GGUF"
    INCLUDE="Qwen2.5-72B-Instruct-Q3_K_XL.gguf"
    REQUIRED_GB="41"
    ;;
  deepseek32-q4km)
    REPO="bartowski/DeepSeek-R1-Distill-Qwen-32B-GGUF"
    INCLUDE="DeepSeek-R1-Distill-Qwen-32B-Q4_K_M.gguf"
    REQUIRED_GB="20"
    ;;
  deepseek70-q4km)
    REPO="bartowski/DeepSeek-R1-Distill-Llama-70B-GGUF"
    INCLUDE="DeepSeek-R1-Distill-Llama-70B-Q4_K_M.gguf"
    REQUIRED_GB="43"
    ;;
  llama70-q4km)
    REPO="bartowski/Meta-Llama-3.1-70B-Instruct-GGUF"
    INCLUDE="Meta-Llama-3.1-70B-Instruct-Q4_K_M.gguf"
    REQUIRED_GB="43"
    ;;
  *)
    echo "Unknown profile: $PROFILE" >&2
    usage >&2
    exit 1 ;;
esac

mkdir -p "$MODEL_DIR"

available_gb() {
  df -Pk "$1" | awk 'NR==2 {printf "%d", $4 / 1024 / 1024}'
}

AVAILABLE_GB="$(available_gb "$MODEL_DIR")"
if [[ "$AVAILABLE_GB" -lt "$REQUIRED_GB" ]]; then
  echo "Insufficient free space in $MODEL_DIR: need about ${REQUIRED_GB} GB, have about ${AVAILABLE_GB} GB." >&2
  echo "Choose a larger --model-dir, free disk space, or select a smaller profile." >&2
  exit 1
fi

if command -v hf >/dev/null 2>&1; then
  HF_CMD=(hf download)
elif command -v huggingface-cli >/dev/null 2>&1; then
  HF_CMD=(huggingface-cli download)
else
  echo "Missing Hugging Face CLI. Install with: pip install -U \"huggingface_hub[cli]\"" >&2
  exit 1
fi

echo "[model] profile=$PROFILE"
echo "[model] repo=$REPO"
echo "[model] include=$INCLUDE"
echo "[model] dir=$MODEL_DIR"
echo "[model] free_space_gb=$AVAILABLE_GB"

if [[ "$PROFILE" == llama70-q4km ]]; then
  echo "[model] note: Meta-Llama downloads may require prior Hugging Face license approval and login." >&2
fi

if [[ "$PROFILE" == deepseek70-q4km ]]; then
  echo "[model] note: DeepSeek 70B distill is Llama-family; current Blazil service is still wired around Qwen2 GGUF loading." >&2
fi

if [[ "$PROFILE" == deepseek32-q4km ]]; then
  echo "[model] note: DeepSeek 32B distill is Qwen-family and is the most plausible DeepSeek candidate for the current inference stack." >&2
fi

"${HF_CMD[@]}" "$REPO" --include "$INCLUDE" --local-dir "$MODEL_DIR"

MODEL_PATH="$MODEL_DIR/$INCLUDE"
if [[ ! -f "$MODEL_PATH" ]]; then
  echo "Download finished but file not found at expected path: $MODEL_PATH" >&2
  exit 1
fi

echo "[model] ready=$MODEL_PATH"
echo "[model] export CLARKENAI_MODEL_PATH=\"$MODEL_PATH\""
echo "[model] next: scripts/clarkenai-70b-bench.sh --model \"$MODEL_PATH\" --label 70b-smoke"
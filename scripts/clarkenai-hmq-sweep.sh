#!/usr/bin/env bash
# Sweep a small set of HMQ profiles against the ClarkenAI cloud benchmark harness.
# Designed for disciplined CPU-only experimentation on the 7B proxy before 70B cloud runs.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BENCH_SCRIPT="$ROOT_DIR/scripts/clarkenai-cloud-bench.sh"
MODEL_PATH="${CLARKENAI_MODEL_PATH:-/Users/rickyanhnguyen/models/Qwen2.5-7B-Instruct-Q4_K_M.gguf}"
RUNS="${CLARKENAI_SWEEP_RUNS:-10}"
PROMPT="${CLARKENAI_SWEEP_PROMPT:-Explain credit risk in one sentence.}"
MAX_TOKENS="${CLARKENAI_SWEEP_MAX_TOKENS:-24}"
THREADS="${CLARKENAI_SWEEP_THREADS:-8}"
WORKERS="${CLARKENAI_SWEEP_WORKERS:-8}"
READY_TIMEOUT_SECS="${CLARKENAI_SWEEP_READY_TIMEOUT_SECS:-180}"
ARTIFACT_ROOT="${CLARKENAI_SWEEP_ARTIFACT_ROOT:-$ROOT_DIR/docs/runs/clarkenai-hmq-sweep}"
TIMESTAMP="$(date +%Y-%m-%d_%H-%M-%S)"
SWEEP_DIR="$ARTIFACT_ROOT/$TIMESTAMP"
SUMMARY_TABLE="$SWEEP_DIR/summary.tsv"
SKIP_BUILD_FIRST="${CLARKENAI_SWEEP_SKIP_BUILD:-false}"

mkdir -p "$SWEEP_DIR"

if [[ ! -x "$BENCH_SCRIPT" ]]; then
  echo "Benchmark script not executable: $BENCH_SCRIPT" >&2
  exit 1
fi

if [[ ! -f "$MODEL_PATH" ]]; then
  echo "Model not found: $MODEL_PATH" >&2
  exit 1
fi

echo -e "label\thybrid\tstage1_end\tstage2_end\ttotal_layers\tlatency_ms_mean\tlatency_ms_p50\tlatency_ms_p95\tlatency_ms_p99\ttokens_per_sec_mean\tartifact_dir" > "$SUMMARY_TABLE"

# label|hybrid_enabled|stage1_end|stage2_end|total_layers|bitnet_threshold
PROFILES=(
  "baseline|false|9|20|28|0.0"
  "balanced|true|10|16|28|0.0"
  "aggressive|true|6|22|28|0.0"
)

BUILT_ONCE="$SKIP_BUILD_FIRST"
for profile in "${PROFILES[@]}"; do
  IFS='|' read -r LABEL HYBRID STAGE1 STAGE2 TOTAL THRESHOLD <<< "$profile"

  echo "[sweep] running profile=$LABEL hybrid=$HYBRID stage1_end=$STAGE1 stage2_end=$STAGE2 total_layers=$TOTAL"

  EXTRA_ARGS=()
  if [[ "$BUILT_ONCE" == true ]]; then
    EXTRA_ARGS+=(--skip-build)
  fi

  "$BENCH_SCRIPT" \
    --model "$MODEL_PATH" \
    --runs "$RUNS" \
    --prompt "$PROMPT" \
    --max-tokens "$MAX_TOKENS" \
    --ready-timeout-secs "$READY_TIMEOUT_SECS" \
    --threads "$THREADS" \
    --workers "$WORKERS" \
    --hybrid-enabled "$HYBRID" \
    --stage1-end "$STAGE1" \
    --stage2-end "$STAGE2" \
    --total-layers "$TOTAL" \
    --bitnet-threshold "$THRESHOLD" \
    --artifact-root "$SWEEP_DIR" \
    --label "$LABEL" \
    "${EXTRA_ARGS[@]}"

  BUILT_ONCE=true

  LATEST_DIR=$(find "$SWEEP_DIR" -maxdepth 1 -type d -name "*_${LABEL}" | sort | tail -n 1)
  if [[ -z "$LATEST_DIR" ]]; then
    echo "[sweep] failed to locate artifact dir for profile=$LABEL" >&2
    exit 1
  fi

  SUMMARY_FILE="$LATEST_DIR/summary.txt"
  if [[ ! -f "$SUMMARY_FILE" ]]; then
    echo "[sweep] missing summary file: $SUMMARY_FILE" >&2
    exit 1
  fi

  LAT_MEAN=$(awk -F= '/^latency_ms_mean=/{print $2}' "$SUMMARY_FILE")
  LAT_P50=$(awk -F= '/^latency_ms_p50=/{print $2}' "$SUMMARY_FILE")
  LAT_P95=$(awk -F= '/^latency_ms_p95=/{print $2}' "$SUMMARY_FILE")
  LAT_P99=$(awk -F= '/^latency_ms_p99=/{print $2}' "$SUMMARY_FILE")
  TPS_MEAN=$(awk -F= '/^tokens_per_sec_mean=/{print $2}' "$SUMMARY_FILE")

  echo -e "${LABEL}\t${HYBRID}\t${STAGE1}\t${STAGE2}\t${TOTAL}\t${LAT_MEAN}\t${LAT_P50}\t${LAT_P95}\t${LAT_P99}\t${TPS_MEAN}\t${LATEST_DIR}" >> "$SUMMARY_TABLE"
done

echo ""
echo "[sweep] complete"
echo "[sweep] summary table: $SUMMARY_TABLE"
cat "$SUMMARY_TABLE"

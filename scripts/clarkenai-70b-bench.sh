#!/usr/bin/env bash
# Production benchmark launcher for ClarkenAI 70B on a co-located CPU-only host.
#
# This wrapper standardizes the runtime knobs we care about for i4i.4xlarge and
# forwards them into the general-purpose cloud benchmark harness.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HARNESS="$ROOT_DIR/scripts/clarkenai-cloud-bench.sh"
MODEL_PATH=""
RUNS=10
PROMPT="Summarize the credit risk outlook for an industrial exporter in two sentences."
MAX_TOKENS=32
READY_TIMEOUT_SECS=300
THREADS=16
WORKERS=16
N_CTX=4096
TEMPERATURE=0.2
OPTIMIZATION_LEVEL=all
HTTP_PORT=8092
if [[ "$(uname -s)" == "Linux" ]]; then
  AERON_DIR="/dev/shm/aeron-inference-hybrid"
else
  AERON_DIR="/tmp/aeron-inference-hybrid"
fi
MODEL_DIR="/tmp/clarkenai-models"
RUN_LABEL="70b-baseline"
ARTIFACT_ROOT="$ROOT_DIR/docs/runs/clarkenai-70b"
HYBRID_ENABLED=true
STAGE1_END=25
STAGE2_END=60
TOTAL_LAYERS=80
BITNET_THRESHOLD=0.0
SKIP_BUILD=false
ALLOW_UNVERIFIED_MODEL="${CLARKENAI_70B_ALLOW_UNVERIFIED_MODEL:-false}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model)
      MODEL_PATH="$2"; shift 2 ;;
    --runs)
      RUNS="$2"; shift 2 ;;
    --prompt)
      PROMPT="$2"; shift 2 ;;
    --max-tokens)
      MAX_TOKENS="$2"; shift 2 ;;
    --ready-timeout-secs)
      READY_TIMEOUT_SECS="$2"; shift 2 ;;
    --threads)
      THREADS="$2"; shift 2 ;;
    --workers)
      WORKERS="$2"; shift 2 ;;
    --n-ctx)
      N_CTX="$2"; shift 2 ;;
    --temperature)
      TEMPERATURE="$2"; shift 2 ;;
    --optimization-level)
      OPTIMIZATION_LEVEL="$2"; shift 2 ;;
    --http-port)
      HTTP_PORT="$2"; shift 2 ;;
    --aeron-dir)
      AERON_DIR="$2"; shift 2 ;;
    --model-dir)
      MODEL_DIR="$2"; shift 2 ;;
    --label)
      RUN_LABEL="$2"; shift 2 ;;
    --artifact-root)
      ARTIFACT_ROOT="$2"; shift 2 ;;
    --hybrid-enabled)
      HYBRID_ENABLED="$2"; shift 2 ;;
    --stage1-end)
      STAGE1_END="$2"; shift 2 ;;
    --stage2-end)
      STAGE2_END="$2"; shift 2 ;;
    --total-layers)
      TOTAL_LAYERS="$2"; shift 2 ;;
    --bitnet-threshold)
      BITNET_THRESHOLD="$2"; shift 2 ;;
    --skip-build)
      SKIP_BUILD=true; shift 1 ;;
    --allow-unverified-model)
      ALLOW_UNVERIFIED_MODEL=true; shift 1 ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 1 ;;
  esac
done

if [[ -z "$MODEL_PATH" ]]; then
  echo "--model is required" >&2
  exit 1
fi

if [[ ! -f "$MODEL_PATH" ]]; then
  echo "Model not found: $MODEL_PATH" >&2
  exit 1
fi

MODEL_BASENAME="$(basename "$MODEL_PATH" | tr '[:upper:]' '[:lower:]')"
if [[ "$ALLOW_UNVERIFIED_MODEL" != true ]] && [[ ! "$MODEL_BASENAME" =~ (^|[^0-9])(70|72)b([^0-9]|$) ]]; then
  echo "Refusing to run scripts/clarkenai-70b-bench.sh with a model path that does not look like a 70B artifact: $MODEL_PATH" >&2
  echo "Use scripts/clarkenai-cloud-bench.sh for proxy-model tuning, or pass --allow-unverified-model / CLARKENAI_70B_ALLOW_UNVERIFIED_MODEL=true if this path is intentional." >&2
  exit 1
fi

ARGS=(
  --model "$MODEL_PATH"
  --runs "$RUNS"
  --prompt "$PROMPT"
  --max-tokens "$MAX_TOKENS"
  --ready-timeout-secs "$READY_TIMEOUT_SECS"
  --threads "$THREADS"
  --workers "$WORKERS"
  --n-ctx "$N_CTX"
  --temperature "$TEMPERATURE"
  --optimization-level "$OPTIMIZATION_LEVEL"
  --http-port "$HTTP_PORT"
  --aeron-dir "$AERON_DIR"
  --model-dir "$MODEL_DIR"
  --artifact-root "$ARTIFACT_ROOT"
  --label "$RUN_LABEL"
  --hybrid-enabled "$HYBRID_ENABLED"
  --stage1-end "$STAGE1_END"
  --stage2-end "$STAGE2_END"
  --total-layers "$TOTAL_LAYERS"
  --bitnet-threshold "$BITNET_THRESHOLD"
)

if [[ "$SKIP_BUILD" == true ]]; then
  ARGS+=(--skip-build)
fi

exec "$HARNESS" "${ARGS[@]}"

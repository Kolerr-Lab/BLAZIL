#!/usr/bin/env bash
# gen-dataset.sh — Generate a synthetic ImageNet-layout dataset for benchmarking.
#
# Creates N 224×224 JPEG images distributed across C synset classes.
# Uses Python (stdlib + optional Pillow) — falls back to raw JPEG bytes if
# Pillow is unavailable. Designed to run on any Linux/macOS server without
# internet access.
#
# Usage:
#   ./scripts/gen-dataset.sh [OPTIONS]
#
# Options:
#   --out DIR        Output root directory (default: /tmp/blazil-imagenet)
#   --images N       Total images to generate (default: 100000)
#   --classes C      Number of synset classes (default: 100)
#   --workers W      Parallel generator workers (default: nproc)
#   --size WxH       Image size (default: 224x224)
#
# Examples:
#   ./scripts/gen-dataset.sh --images 1000000 --out /data/imagenet-bench
#   ./scripts/gen-dataset.sh --images 10000 --out /tmp/quick-test

set -euo pipefail

# ─── Defaults ────────────────────────────────────────────────────────────────
OUT_DIR="/tmp/blazil-imagenet"
TOTAL_IMAGES=100000
NUM_CLASSES=100
WORKERS=$(nproc 2>/dev/null || sysctl -n hw.logicalcpu 2>/dev/null || echo 4)
IMAGE_W=224
IMAGE_H=224

# ─── Parse args ──────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)      OUT_DIR="$2";        shift 2 ;;
    --images)   TOTAL_IMAGES="$2";   shift 2 ;;
    --classes)  NUM_CLASSES="$2";    shift 2 ;;
    --workers)  WORKERS="$2";        shift 2 ;;
    --size)     IFS='x' read -r IMAGE_W IMAGE_H <<< "$2"; shift 2 ;;
    *) echo "Unknown arg: $1"; exit 1 ;;
  esac
done

echo ""
echo "  ════════════════════════════════════════════════════════"
echo "  Blazil Synthetic Dataset Generator"
echo "  ════════════════════════════════════════════════════════"
echo "  Output dir  : ${OUT_DIR}"
echo "  Total images: ${TOTAL_IMAGES}"
echo "  Classes     : ${NUM_CLASSES}"
echo "  Workers     : ${WORKERS}"
echo "  Image size  : ${IMAGE_W}x${IMAGE_H}"
echo "  ────────────────────────────────────────────────────────"

# ─── Check Python ────────────────────────────────────────────────────────────
PYTHON=$(command -v python3 || command -v python || echo "")
if [[ -z "$PYTHON" ]]; then
  echo "ERROR: python3 not found. Install with: sudo apt install python3" >&2
  exit 1
fi

# ─── Generate ────────────────────────────────────────────────────────────────
START_TIME=$(date +%s)

$PYTHON - <<PYEOF
import os, sys, random, struct, math, multiprocessing, time

out_dir   = "${OUT_DIR}"
total     = ${TOTAL_IMAGES}
classes   = ${NUM_CLASSES}
workers   = ${WORKERS}
img_w     = ${IMAGE_W}
img_h     = ${IMAGE_H}

# Try to import Pillow for realistic JPEG generation
try:
    from PIL import Image
    import io
    HAS_PIL = True
except ImportError:
    HAS_PIL = False

# Synset IDs: n00000000 .. n00000NNN
synsets = [f"n{i:08d}" for i in range(classes)]
per_class = math.ceil(total / classes)

def make_jpeg_bytes_pil(w, h, seed):
    """Generate a realistic random JPEG using Pillow."""
    rng = random.Random(seed)
    pixels = bytes([rng.randint(0, 255) for _ in range(w * h * 3)])
    img = Image.frombytes('RGB', (w, h), pixels)
    buf = io.BytesIO()
    img.save(buf, format='JPEG', quality=85, optimize=False)
    return buf.getvalue()

# Minimal valid 1x1 JPEG — used as fallback template, padded to target size
# We embed dimensions in APP0 comment for differentiation per image
MINIMAL_JPEG = bytes([
    0xFF,0xD8,0xFF,0xE0,0x00,0x10,0x4A,0x46,0x49,0x46,0x00,0x01,0x01,0x00,0x00,0x01,
    0x00,0x01,0x00,0x00,0xFF,0xDB,0x00,0x43,0x00,0x08,0x06,0x06,0x07,0x06,0x05,0x08,
    0x07,0x07,0x07,0x09,0x09,0x08,0x0A,0x0C,0x14,0x0D,0x0C,0x0B,0x0B,0x0C,0x19,0x12,
    0x13,0x0F,0x14,0x1D,0x1A,0x1F,0x1E,0x1D,0x1A,0x1C,0x1C,0x20,0x24,0x2E,0x27,0x20,
    0x22,0x2C,0x23,0x1C,0x1C,0x28,0x37,0x29,0x2C,0x30,0x31,0x34,0x34,0x34,0x1F,0x27,
    0x39,0x3D,0x38,0x32,0x3C,0x2E,0x33,0x34,0x32,0xFF,0xC0,0x00,0x0B,0x08,0x00,0x01,
    0x00,0x01,0x01,0x01,0x11,0x00,0xFF,0xC4,0x00,0x1F,0x00,0x00,0x01,0x05,0x01,0x01,
    0x01,0x01,0x01,0x01,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x01,0x02,0x03,0x04,
    0x05,0x06,0x07,0x08,0x09,0x0A,0x0B,0xFF,0xDA,0x00,0x08,0x01,0x01,0x00,0x00,0x3F,
    0x00,0xFB,0x28,0xA2,0x80,0x1F,0xFF,0xD9
])

def make_jpeg_bytes_raw(w, h, seed):
    """
    Fallback: embed a JFIF comment with random data to create a unique file
    per image. File size ~= 2KB regardless of w/h, but sufficient for
    I/O throughput benchmarking of the transport layer.
    """
    rng = random.Random(seed)
    # Build a JFIF APP1 comment block with random payload
    comment_payload = bytes([rng.randint(0, 255) for _ in range(w * h * 3 // 10)])
    comment_len = len(comment_payload) + 2
    comment_block = b'\xFF\xFE' + struct.pack('>H', comment_len) + comment_payload
    # Insert after SOI+APP0 (first 20 bytes), before the rest
    data = MINIMAL_JPEG[:20] + comment_block + MINIMAL_JPEG[20:]
    return data

make_jpeg = make_jpeg_bytes_pil if HAS_PIL else make_jpeg_bytes_raw

def generate_class(args):
    synset, class_idx, n_images, w, h = args
    cls_dir = os.path.join(out_dir, "train", synset)
    os.makedirs(cls_dir, exist_ok=True)
    count = 0
    for i in range(n_images):
        seed = class_idx * 100000 + i
        path = os.path.join(cls_dir, f"{synset}_{i:06d}.JPEG")
        if os.path.exists(path):
            count += 1
            continue
        data = make_jpeg(w, h, seed)
        with open(path, 'wb') as f:
            f.write(data)
        count += 1
    return count

print(f"  PIL available: {HAS_PIL}")
print(f"  Generating {total} images across {classes} classes...")
print(f"  Output: {out_dir}/train/")
sys.stdout.flush()

tasks = [(synsets[i], i, per_class, img_w, img_h) for i in range(classes)]
total_written = 0
t_start = time.time()

pool = multiprocessing.Pool(processes=workers)
for n in pool.imap_unordered(generate_class, tasks):
    total_written += n
    elapsed = time.time() - t_start
    rate = total_written / elapsed if elapsed > 0 else 0
    print(f"\r  Progress: {total_written}/{total} ({rate:.0f} img/s)  ", end="", flush=True)
pool.close()
pool.join()

elapsed = time.time() - t_start
rate = total_written / elapsed if elapsed > 0 else 0
print(f"\r  Done: {total_written} images in {elapsed:.1f}s ({rate:.0f} img/s)       ")
PYEOF

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

echo "  ────────────────────────────────────────────────────────"
echo "  ✓  Dataset ready at: ${OUT_DIR}/train/"
echo "  ✓  Time: ${ELAPSED}s"
echo ""
echo "  Run benchmark:"
echo "    ./target/release/ml-bench \\"
echo "      --mode dataloader \\"
echo "      --dataset imagenet \\"
echo "      --path ${OUT_DIR} \\"
echo "      --batch-size 256 \\"
echo "      --num-workers 16 \\"
echo "      --duration 120 \\"
echo "      --metrics-port 9092"
echo ""
echo "  With fault injection:"
echo "    ./target/release/ml-bench \\"
echo "      --mode dataloader \\"
echo "      --dataset imagenet \\"
echo "      --path ${OUT_DIR} \\"
echo "      --batch-size 256 \\"
echo "      --num-workers 16 \\"
echo "      --duration 120 \\"
echo "      --fault-mode disk_unplug \\"
echo "      --fault-at 30 \\"
echo "      --fault-duration 10 \\"
echo "      --metrics-port 9092"
echo "  ════════════════════════════════════════════════════════"

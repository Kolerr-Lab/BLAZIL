# Blazil Media Plane (blazil-media) — Railway service image.
#
# IMPORTANT: build CONTEXT must be the Blazil workspace ROOT (this crate is a workspace
# member with a path dependency on core/transport and inherits [workspace.dependencies]).
# On Railway set:  Root Directory = /   ·   Dockerfile Path = services/media/Dockerfile
#
#   docker build -f services/media/Dockerfile -t blazil-media .   # run from workspace root

# ---- builder ----------------------------------------------------------------
FROM rust:1.88-slim-bookworm AS builder

# TLS stack (rustls/aws-lc + ring) and native build deps.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential cmake perl pkg-config protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Copy the whole workspace so path deps + Cargo.lock resolve.
COPY . .

# Build only the media crate in release mode.
RUN cargo build -p blazil-media --release --locked \
    && cp target/release/blazil-media /blazil-media

# ---- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# CA certs for outbound TLS to ElevenLabs + the backend; curl to fetch the endpoint model.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Predictive endpointing (Bước 3): bake the Smart Turn v2 ONNX model so runtime never downloads.
# NOTE (Copilot): verify the exact HuggingFace file path/name for the ONNX export before relying
# on this — adjust the URL/filename if the repo layout differs.
RUN mkdir -p /opt/models/smart-turn-v2 \
    && curl -fsSL -o /opt/models/smart-turn-v2/model.onnx \
       https://huggingface.co/pipecat-ai/smart-turn-v2/resolve/main/model.onnx

COPY --from=builder /blazil-media /usr/local/bin/blazil-media
# Ensure libonnxruntime.so downloaded by ort is available at runtime.
COPY --from=builder /build/target/release/libonnxruntime.so* /usr/lib/
# Railway injects $PORT; the binary reads it (falls back to 8080).
ENV RUST_LOG=blazil_media=info,tower_http=info
CMD ["/usr/local/bin/blazil-media"]

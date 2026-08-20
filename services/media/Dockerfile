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
        build-essential cmake perl pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Copy the whole workspace so path deps + Cargo.lock resolve.
COPY . .

# Build only the media crate in release mode.
RUN cargo build -p blazil-media --release --locked \
    && cp target/release/blazil-media /blazil-media

# ---- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# CA certs for outbound TLS to ElevenLabs + the backend.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /blazil-media /usr/local/bin/blazil-media

# Railway injects $PORT; the binary reads it (falls back to 8080).
ENV RUST_LOG=blazil_media=info,tower_http=info
CMD ["/usr/local/bin/blazil-media"]

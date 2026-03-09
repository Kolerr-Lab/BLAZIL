#!/usr/bin/env bash

set -euo pipefail

# Blazil Development Environment Setup Script
# For macOS Apple Silicon (ARM64)

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Blazil Development Environment Setup"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Helper functions
print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

# Check if running on macOS
if [[ "$OSTYPE" != "darwin"* ]]; then
    print_error "This script is designed for macOS. Detected: $OSTYPE"
    exit 1
fi

# Check if running on Apple Silicon
if [[ $(uname -m) != "arm64" ]]; then
    print_warning "This script is optimized for Apple Silicon (ARM64). Detected: $(uname -m)"
fi

print_status "Running on macOS Apple Silicon"
echo ""

# Check for Homebrew
echo "Checking for Homebrew..."
if ! command -v brew &> /dev/null; then
    print_error "Homebrew not found. Installing Homebrew..."
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    eval "$(/opt/homebrew/bin/brew shellenv)"
else
    print_status "Homebrew installed"
fi
echo ""

# Check for Rust
echo "Checking for Rust toolchain..."
if ! command -v rustc &> /dev/null; then
    print_error "Rust not found. Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    print_status "Rust installed: $(rustc --version)"
fi

# Ensure we have the correct Rust target
echo "Adding aarch64-apple-darwin target..."
rustup target add aarch64-apple-darwin
print_status "Rust target configured"
echo ""

# Check for Go
echo "Checking for Go..."
if ! command -v go &> /dev/null; then
    print_error "Go not found. Installing Go 1.22..."
    brew install go@1.22
else
    GO_VERSION=$(go version | awk '{print $3}' | sed 's/go//')
    if [[ "$GO_VERSION" < "1.22" ]]; then
        print_warning "Go version $GO_VERSION is older than 1.22. Upgrading..."
        brew upgrade go
    else
        print_status "Go installed: $(go version)"
    fi
fi
echo ""

# Check for Docker
echo "Checking for Docker..."
if ! command -v docker &> /dev/null; then
    print_error "Docker not found. Please install Docker Desktop from: https://www.docker.com/products/docker-desktop"
    exit 1
else
    print_status "Docker installed: $(docker --version)"
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    print_error "Docker daemon is not running. Please start Docker Desktop."
    exit 1
else
    print_status "Docker daemon is running"
fi
echo ""

# Check for Docker Compose
echo "Checking for Docker Compose..."
if ! docker compose version &> /dev/null; then
    print_error "Docker Compose not found or not working"
    exit 1
else
    print_status "Docker Compose installed: $(docker compose version)"
fi
echo ""

# Build Rust workspace
echo "Building Rust workspace..."
if cargo build --workspace; then
    print_status "Rust workspace built successfully"
else
    print_error "Failed to build Rust workspace"
    exit 1
fi
echo ""

# Tidy Go modules
echo "Tidying Go modules..."
cd services
for service in gateway payments banking trading crypto compliance; do
    echo "  Processing $service..."
    cd "$service"
    go mod tidy
    cd ..
done
cd ..
print_status "Go modules tidied"
echo ""

# Pull Docker images
echo "Pulling Docker images (this may take a few minutes)..."
if docker compose -f infra/docker/docker-compose.dev.yml pull; then
    print_status "Docker images pulled successfully"
else
    print_warning "Failed to pull some Docker images. They will be pulled on first run."
fi
echo ""

# Summary
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Setup Complete! 🚀"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Next steps:"
echo ""
echo "  1. Start development infrastructure:"
echo "     docker compose -f infra/docker/docker-compose.dev.yml up -d"
echo ""
echo "  2. Build and test all components:"
echo "     ./scripts/check.sh"
echo ""
echo "  3. Run benchmarks:"
echo "     ./scripts/bench.sh"
echo ""
echo "  4. View logs:"
echo "     docker compose -f infra/docker/docker-compose.dev.yml logs -f"
echo ""
echo "  5. Access services:"
echo "     - Redpanda Console: http://localhost:8080"
echo "     - Grafana: http://localhost:3001 (admin/admin)"
echo "     - Prometheus: http://localhost:9090"
echo "     - Vault: http://localhost:8200 (token: blazil-dev-token)"
echo "     - Keycloak: http://localhost:8180 (admin/admin)"
echo ""
print_status "Happy coding!"

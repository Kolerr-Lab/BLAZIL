#!/usr/bin/env bash

set -euo pipefail

# Blazil Quality Check Script
# Runs linting, testing, and security audits

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Blazil Quality Checks"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

FAILED=0

print_status() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
    FAILED=1
}

print_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

print_section() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "$1"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
}

# Rust checks
print_section "Rust Checks"

print_info "Running cargo fmt..."
if cargo fmt --all -- --check; then
    print_status "Rust formatting is correct"
else
    print_error "Rust formatting issues found. Run: cargo fmt --all"
fi

print_info "Running cargo clippy..."
if cargo clippy --workspace --all-targets --all-features -- -D warnings; then
    print_status "Clippy checks passed"
else
    print_error "Clippy found issues"
fi

print_info "Running cargo check..."
if cargo check --workspace --all-features; then
    print_status "Cargo check passed"
else
    print_error "Cargo check failed"
fi

print_info "Running cargo test..."
if cargo test --workspace --all-features; then
    print_status "Rust tests passed"
else
    print_error "Rust tests failed"
fi

print_info "Running cargo audit..."
if command -v cargo-audit &> /dev/null; then
    if cargo audit; then
        print_status "No security vulnerabilities found"
    else
        print_error "Security vulnerabilities found"
    fi
else
    echo -e "${YELLOW}⚠${NC} cargo-audit not installed. Skipping security audit."
    echo "  Install with: cargo install cargo-audit"
fi

# Go checks
print_section "Go Checks"

cd services

for service in gateway payments banking trading crypto compliance; do
    print_info "Checking $service..."
    
    cd "$service"
    
    # Format check
    if ! gofmt -l . | grep -q .; then
        print_status "$service: formatting is correct"
    else
        print_error "$service: formatting issues found. Run: go fmt ./..."
    fi
    
    # Build
    if go build -v ./...; then
        print_status "$service: build succeeded"
    else
        print_error "$service: build failed"
    fi
    
    # Test
    if go test -v ./...; then
        print_status "$service: tests passed"
    else
        print_error "$service: tests failed"
    fi
    
    cd ..
done

cd ..

# Docker Compose validation
print_section "Infrastructure Checks"

print_info "Validating docker-compose.dev.yml..."
if docker compose -f infra/docker/docker-compose.dev.yml config > /dev/null; then
    print_status "docker-compose.dev.yml is valid"
else
    print_error "docker-compose.dev.yml has errors"
fi

print_info "Validating docker-compose.test.yml..."
if docker compose -f infra/docker/docker-compose.test.yml config > /dev/null; then
    print_status "docker-compose.test.yml is valid"
else
    print_error "docker-compose.test.yml has errors"
fi

# Summary
print_section "Summary"

if [ $FAILED -eq 0 ]; then
    print_status "All quality checks passed! 🎉"
    exit 0
else
    print_error "Some quality checks failed. Please fix the issues above."
    exit 1
fi

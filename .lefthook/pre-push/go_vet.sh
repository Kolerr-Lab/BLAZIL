#!/usr/bin/env bash
# .lefthook/pre-push/go_vet.sh
#
# Run `go vet ./...` on every Go module in the Blazil monorepo.
# Called by Lefthook on `git push` (pre-push hook).
#
# Each module uses GOWORK=off so that go.mod replace directives are
# resolved via their relative paths — identical to CI behaviour.

set -euo pipefail

MODULES=(
  services/gateway
  services/payments
  services/banking
  services/trading
  services/crypto
  libs/metering
  libs/auth
  libs/discovery
  libs/observability
  libs/policy
  libs/secrets
  libs/sharding
)

ROOT="$(git rev-parse --show-toplevel)"
FAILED=0

for mod in "${MODULES[@]}"; do
  printf "  vet %-40s" "${mod}"
  if (cd "${ROOT}/${mod}" && GOWORK=off go vet ./... 2>&1); then
    echo "ok"
  else
    echo "FAIL"
    FAILED=1
  fi
done

exit "${FAILED}"

## Description

<!-- What problem does this PR solve? Link to the issue it addresses. -->

Fixes #

## Type of Change

- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Performance improvement
- [ ] Refactoring / cleanup
- [ ] Documentation / runbook
- [ ] CI / infra / tooling

## What Changed

<!-- Bullet-point summary of the key changes. Include any architecture decisions. -->

-

## Testing

| Test type | Command | Status |
|-----------|---------|--------|
| Rust unit | `cargo test --workspace` | [ ] |
| Go unit | `cd services && go test ./...` | [ ] |
| Integration | `cargo test -p blazil-ledger --features tigerbeetle-client` | [ ] |
| Benchmarks | `./scripts/bench.sh` | [ ] N/A |

Describe any manual verification steps performed:

## Security & Compliance

- [ ] No secrets / credentials committed
- [ ] OWASP Top 10 considered (injection, auth, SSRF, etc.)
- [ ] `cargo audit` / `trivy` pass with no new CRITICAL/HIGH findings
- [ ] OPA policy rules updated if access control changed (`infra/policies/blazil.rego`)

## Observability

- [ ] New code paths emit structured logs (`tracing::info!` / `tracing::error!`)
- [ ] New metrics registered and exposed on `/metrics`
- [ ] Grafana dashboard updated if new panels are needed

## Deployment Notes

<!-- Anything the reviewer or on-call needs to know when this ships:
     - env vars added/changed
     - migration steps
     - feature flags
     - rollback procedure -->

## Checklist

- [ ] PR title follows `scope: short description` convention (e.g. `engine: add risk limit config`)
- [ ] Self-review completed — code reads clearly without inline explanation
- [ ] Docs/runbooks updated (`docs/`, `infra/docker/`, `observability/`)
- [ ] CHANGELOG.md updated under `[Unreleased]`
- [ ] No regressions in `cargo clippy --workspace -- -D warnings`

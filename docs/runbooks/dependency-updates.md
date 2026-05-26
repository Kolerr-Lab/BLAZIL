# Runbook: Dependency Updates

**Severity:** Standard change  
**Estimated time:** 30–60 minutes  
**Requires:** Rust + Go toolchains, CI write access, branch permissions

---

## Overview

Dependencies are updated on a **monthly cadence** (or immediately for high/critical CVEs).  
All updates go through a PR with CI validation before merging to `main`.

---

## 1. Rust dependency updates

### 1a. Check for outdated dependencies

```bash
cd /path/to/repo
cargo outdated          # shows semver-compatible and breaking updates
cargo audit             # shows known CVEs
```

### 1b. Apply semver-compatible updates

```bash
# Update all dependencies within semver constraints
cargo update

# Verify no breakage
cargo check --workspace
cargo test --workspace
```

### 1c. Apply breaking updates (major version bumps)

Review each dependency's migration guide individually. Common ones:

| Crate | Migration notes |
|-------|----------------|
| `tokio` | Check async API changes in release notes |
| `rmp-serde` | Verify ARRAY serialisation format compatibility |
| `axum` | Router API changes each major version |

```bash
# Edit Cargo.toml to bump the specific dependency
# Then:
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

### 1d. Address CVEs

```bash
cargo audit fix          # auto-patches minor CVEs where possible
cargo audit              # re-run to confirm resolution
```

For CVEs that cannot be auto-fixed, file an issue and either:
- Upgrade the affected crate manually, or
- Apply a `[patch]` override if upstream has a fix not yet published.

---

## 2. Go dependency updates

```bash
cd services/

# Check for available updates
go list -u -m all 2>/dev/null | grep '\['

# Update all modules within semver bounds
go get -u ./...
go mod tidy

# Verify
go build ./...
go test ./...
```

For each Go service directory:

```bash
for svc in banking crypto gateway inference payments trading; do
  echo "=== $svc ==="
  cd $svc
  go get -u ./...
  go mod tidy
  go build ./...
  cd ..
done
```

---

## 3. Container base image updates

```bash
# Scan current images for CVEs
trivy image ghcr.io/kolerr-lab/blazil-engine:latest
trivy image ghcr.io/kolerr-lab/blazil-gateway:latest

# Update base image tags in Dockerfiles
# e.g. FROM rust:1.88-slim → FROM rust:1.89-slim
# Rebuild and re-scan
```

All CI-built images are automatically scanned via the `trivy-scan` step in `.github/workflows/ci.yml`.

---

## 4. JavaScript / tooling dependencies (dashboards)

```bash
cd tools/fintech-dashboard
npm update
npm audit fix
npm test

cd tools/ai-dashboard
npm update
npm audit fix
npm test
```

---

## 5. Commit and PR

```bash
git checkout -b deps/$(date +%Y-%m)

git add Cargo.lock services/*/go.sum services/go.work.sum
git commit -m "chore(deps): monthly dependency update $(date +%Y-%m)"

git push origin deps/$(date +%Y-%m)
# Open PR — CI must pass before merge
```

---

## CVE triage priority

| CVSS score | Action | Deadline |
|------------|--------|----------|
| 9.0–10.0 (Critical) | Immediate fix + hotfix deploy | Same business day |
| 7.0–8.9 (High) | Fix in next sprint | Within 7 days |
| 4.0–6.9 (Medium) | Include in monthly update | Within 30 days |
| < 4.0 (Low) | Backlog | Best effort |

---

## References

- `Cargo.toml` — workspace dependency declarations
- `services/go.work` — Go workspace
- `.github/workflows/ci.yml` — `cargo-audit` and `trivy-scan` steps
- [RustSec Advisory DB](https://rustsec.org)
- [Go Vulnerability DB](https://pkg.go.dev/vuln/)

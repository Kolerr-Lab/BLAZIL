# Runbook: Version Upgrade

**Severity:** Standard change  
**Estimated time:** 1–2 hours depending on scope  
**Requires:** `kubectl` with prod context, Cargo/Go toolchains, CI write access

---

## Scope

This runbook covers upgrading core runtime and framework versions:

- Rust toolchain (MSRV bump)
- Go toolchain
- TigerBeetle server binary
- Kubernetes / cert-manager / OTel operator
- Major dependency upgrades (e.g. `tokio`, `vmihailenco/msgpack`)

For routine patch-level dependency updates, see `dependency-updates.md`.

---

## 1. Rust toolchain upgrade

### 1a. Update MSRV

Edit `Cargo.toml` (workspace root):

```toml
[workspace.package]
rust-version = "1.XX.0"   # new MSRV
```

Edit `.github/workflows/ci.yml` to match the new toolchain version.

### 1b. Verify compilation

```bash
rustup update stable
cargo check --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

### 1c. Update `rust-toolchain.toml` (if present)

```toml
[toolchain]
channel = "1.XX.0"
```

### 1d. Audit for new deprecations

```bash
cargo audit
```

Resolve any new advisories before proceeding.

---

## 2. Go toolchain upgrade

```bash
cd services/

# Update go.work toolchain directive
go work edit -toolchain go1.XX.X

# Update each module
for mod in banking crypto gateway inference payments trading; do
  cd $mod
  go mod tidy
  cd ..
done

# Build all
go build ./...
go test ./...
```

---

## 3. TigerBeetle server upgrade

TigerBeetle versions are tightly coupled to the client library version. Both must be upgraded together.

1. Check the new TigerBeetle release notes for breaking protocol changes.
2. Update `tigerbeetle-unofficial-core` version in `core/ledger/Cargo.toml`.
3. Run `cargo check -p blazil-ledger` to confirm API compatibility.
4. Build new TigerBeetle Docker image:

```bash
cd infra/docker
docker build -f Dockerfile.tigerbeetle \
  --build-arg TB_VERSION=0.XX.XX \
  -t ghcr.io/kolerr-lab/blazil-tigerbeetle:0.XX.XX .
```

5. Deploy via blue-green process (see `blue-green-deployment.md`).

> **Note:** TigerBeetle cannot roll back a data file to an older version. Ensure a backup snapshot exists before upgrading (see `backup-restore.md`).

---

## 4. Kubernetes / cert-manager upgrade

```bash
# Check current versions
kubectl version
kubectl -n cert-manager get deployment cert-manager \
  -o jsonpath='{.spec.template.spec.containers[0].image}'

# Apply new manifests (after testing in staging)
kubectl apply -f https://github.com/cert-manager/cert-manager/releases/download/vX.X.X/cert-manager.yaml

# Verify
kubectl -n cert-manager rollout status deployment/cert-manager --timeout=120s
kubectl get clusterissuer blazil-internal-ca -o jsonpath='{.status.conditions[0].type}'
# Expected: Ready
```

---

## 5. Post-upgrade validation

```bash
# Full test suite
cargo test --workspace
cd services && go test ./...

# Integration smoke test
cargo test --package blazil-bench -- smoke --ignored

# Security scan
cargo audit
trivy image ghcr.io/kolerr-lab/blazil-engine:latest

# Check all pods healthy
kubectl -n blazil get pods
```

---

## Rollback

For Kubernetes component upgrades: apply the previous manifest version.  
For Rust/Go upgrades: revert `Cargo.toml` / `go.work` changes and redeploy previous image tag.  
For TigerBeetle: restore from pre-upgrade snapshot (see `backup-restore.md`).

---

## Post-upgrade checklist

- [ ] All CI checks pass on the new toolchain
- [ ] `CHANGELOG.md` updated with version changes
- [ ] `README.md` badges updated if applicable
- [ ] Security advisories in `cargo audit` / `trivy` output resolved
- [ ] Staging deployed and validated before production

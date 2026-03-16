# Contributing to Blazil

Blazil is an open-source financial infrastructure project. We welcome contributions of all kinds — bug fixes, performance improvements, new payment rails, documentation, and more.

---

## Table of Contents

- [Before you start](#before-you-start)
- [Development setup](#development-setup)
- [Making changes](#making-changes)
- [Testing](#testing)
- [Pull request process](#pull-request-process)
- [Code style](#code-style)
- [Performance discipline](#performance-discipline)
- [Commit conventions](#commit-conventions)

---

## Before you start

- Check [open issues](https://github.com/Kolerr-Lab/BLAZIL/issues) before filing a new one.
- For large changes, open an issue first to discuss the approach.
- For security vulnerabilities, do **not** open a public issue — see [SECURITY.md](SECURITY.md).
- By contributing you agree your work will be licensed under Apache 2.0.

---

## Development setup

### Prerequisites

| Tool | Minimum version |
|------|-----------------|
| Rust | stable (2021 edition) |
| Go | 1.22 |
| Docker | 24.x |
| Docker Compose | 2.x |

### First time

```bash
git clone https://github.com/Kolerr-Lab/BLAZIL.git
cd BLAZIL
./scripts/setup.sh           # installs toolchains, lints, hooks
```

### Start the dev stack

```bash
docker compose -f infra/docker/docker-compose.dev.yml up -d
```

This starts TigerBeetle (single replica), Prometheus, Grafana, and Redis locally.

### Build everything

```bash
cargo build --workspace                  # Rust core
cd services && go build ./...            # Go services
```

---

## Making changes

```bash
git checkout -b feat/my-feature   # or fix/issue-123
# ... make changes ...
./scripts/check.sh                # must pass before pushing
git commit -m 'feat(payments): add SEPA credit transfer'
git push origin feat/my-feature
```

Open a pull request against `main`. Fill out the PR template completely.

---

## Testing

```bash
# All Rust tests
cargo test --workspace

# All Go tests
cd services && go test ./...

# Full check (lint + test + audit)
./scripts/check.sh
```

Tests that require a live TigerBeetle instance read `BLAZIL_TB_ADDRESS` from the environment and skip automatically when unset.

For performance-sensitive changes, include benchmark output in your PR:

```bash
cargo bench -p blazil-bench             # Criterion micro-benchmarks
cargo run -p blazil-bench --release     # 4-scenario load harness
```

---

## Pull request process

1. CI must be green (build, test, lint, audit).
2. Include a description of **what** changed and **why**.
3. For hot-path changes, include before/after benchmark numbers.
4. Squash fixup commits before requesting review.
5. One approval from a maintainer required to merge.

Expect a review within 3 business days.

---

## Code style

### Rust
- `cargo fmt` before every commit (enforced by CI)
- `cargo clippy -- -D warnings` must pass
- Public APIs require doc comments
- Unit tests live in the same file (`#[cfg(test)]`)
- Integration tests go in `tests/`

### Go
- `gofmt` / `goimports` formatting
- `golangci-lint run` must pass
- Use `slog` for structured logging (not `fmt.Println`)
- Table-driven tests preferred

### General
- Atomic, focused commits — one logical change per commit
- No dead code, no commented-out blocks
- Update documentation when changing observable behaviour

---

## Performance discipline

Blazil targets sub-millisecond engine latency and 60K+ TPS at the system level. When touching hot paths:

1. **Profile first** — use `cargo bench`, `perf`, or `pprof` to identify the actual bottleneck before changing anything.
2. **No allocations in the pipeline** — the Disruptor ring buffer path must remain allocation-free. Use pre-allocated buffers and avoid `Box`, `Vec::new()`, or `String` in hot loops.
3. **Benchmark regressions are blockers** — a PR that regresses throughput by >2% or P99 by >5% will not merge without a compelling reason.
4. **Document the trade-off** — if you sacrifice latency for throughput (or vice versa), say so explicitly in the PR.
5. **Lock-free where possible** — prefer atomics over mutexes in hot paths.

---

## Commit conventions

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>

[optional body]

[optional footer]
```

| Type | When to use |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `perf` | Performance improvement |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `test` | Adding or correcting tests |
| `docs` | Documentation only |
| `ci` | CI configuration |
| `chore` | Build scripts, dependency bumps |

Scopes: `engine`, `transport`, `ledger`, `risk`, `payments`, `banking`, `trading`, `crypto`, `bench`, `stresstest`, `infra`, `observability`, `docs`.

### Examples

```
feat(payments): add SEPA credit transfer rail
fix(engine): release ring buffer slot on batch timeout
perf(transport): replace Mutex with RwLock in metrics server
docs(readme): add roadmap and benchmark results
```

---

## Questions?

Open a [GitHub Discussion](https://github.com/Kolerr-Lab/BLAZIL/discussions) or an issue. We're happy to help.


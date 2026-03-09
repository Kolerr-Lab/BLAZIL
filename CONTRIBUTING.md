# Contributing to Blazil

Thank you for your interest in contributing to Blazil! We welcome contributions from the community.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Making Changes](#making-changes)
- [Submitting a Pull Request](#submitting-a-pull-request)
- [Code Style](#code-style)
- [Testing](#testing)
- [Performance Considerations](#performance-considerations)

## Code of Conduct

By participating in this project, you agree to treat all contributors with respect. We are committed to providing a welcoming and inclusive environment.

## Getting Started

1. Fork the repository on GitHub
2. Clone your fork locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/blazil.git
   cd blazil
   ```
3. Set up the development environment:
   ```bash
   ./scripts/setup.sh
   ```

## Development Setup

### Prerequisites

- Rust (stable, edition 2021)
- Go 1.22+
- Docker and Docker Compose
- macOS Apple Silicon or Linux aarch64/x86_64

### Start the development stack:
```bash
docker compose -f infra/docker/docker-compose.dev.yml up -d
```

## Making Changes

1. Create a new branch for your feature or bug fix:
   ```bash
   git checkout -b feature/my-feature
   # or
   git checkout -b fix/issue-123
   ```

2. Make your changes following the code style guidelines

3. Run quality checks before committing:
   ```bash
   ./scripts/check.sh
   ```

4. Commit your changes with descriptive messages:
   ```bash
   git commit -m "feat(engine): implement Disruptor-based pipeline"
   ```

## Submitting a Pull Request

1. Push your branch to your fork:
   ```bash
   git push origin feature/my-feature
   ```

2. Open a pull request on GitHub against the `main` branch

3. Fill out the pull request template completely

4. Wait for CI to pass and a maintainer to review

## Code Style

### Rust
- Follow `rustfmt` formatting (`cargo fmt`)
- Pass `clippy` checks (`cargo clippy`)
- Write documentation for public APIs
- Include unit tests for new functionality

### Go
- Follow `gofmt` formatting
- Follow Go idioms and conventions
- Write tests with `go test`
- Use structured logging

### General
- Keep commits atomic and focused
- Write meaningful commit messages (follow Conventional Commits)
- Update documentation when changing behavior
- Do not break existing tests

## Testing

### Rust
```bash
cargo test --workspace
```

### Go
```bash
cd services
for svc in gateway payments banking trading crypto compliance; do
    cd $svc && go test ./... && cd ..
done
```

### All checks
```bash
./scripts/check.sh
```

## Performance Considerations

Blazil targets 1M-10M TPS. When contributing:

1. **Profile before optimizing** — use `cargo bench` and `pprof`
2. **Avoid allocations in hot paths** — use pre-allocated buffers
3. **Benchmark regressions are blockers** — include benchmark results in PRs that touch hot paths
4. **Use lock-free data structures** where possible
5. **Document performance characteristics** of new code

## Questions?

Open an issue or start a discussion on GitHub.

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.

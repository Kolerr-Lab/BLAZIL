# Changelog

All notable changes to Blazil will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial monorepo skeleton with Rust core workspace and Go services
- Core Rust crates: `blazil-engine`, `blazil-transport`, `blazil-ledger`, `blazil-risk`, `blazil-common`
- Go microservices: gateway, payments, banking, trading, crypto, compliance
- Docker Compose development stack (TigerBeetle, Redpanda, Vault, Keycloak, OTel, Prometheus, Grafana)
- GitHub Actions CI pipeline (Rust + Go)
- Benchmark regression checks workflow
- Security scanning workflow (Trivy + cargo audit)
- Architecture documentation and ADRs
- Setup, benchmark, and quality check scripts

---

<!-- 
Format for entries:

## [X.Y.Z] - YYYY-MM-DD

### Added
- New feature description

### Changed
- Changed feature description

### Deprecated
- Deprecated feature description

### Removed
- Removed feature description

### Fixed
- Bug fix description

### Security
- Security fix description
-->

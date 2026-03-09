# Architecture Decision Record: Monorepo Structure

## Status

Accepted

## Context

Blazil is a complex system spanning multiple programming languages (Rust for core performance-critical components, Go for business services) and requires coordination between:

- Ultra-low-latency transaction processing engine (Rust)
- Transport and ingestion layers (Rust with io_uring)
- Multiple domain services (Go microservices)
- Shared infrastructure and tooling
- Comprehensive testing and benchmarking

We need a code organization strategy that:
1. Enables atomic commits across component boundaries
2. Simplifies dependency management
3. Maintains clear boundaries between modules
4. Supports independent deployment of services
5. Facilitates comprehensive CI/CD pipelines

## Decision

We will use a **monorepo structure** with the following organization:

### Core Components (Rust)
All performance-critical, low-level components in `core/`:
- `engine/` - Transaction processing engine
- `transport/` - Network ingestion layer
- `ledger/` - TigerBeetle client abstractions
- `risk/` - Pre-trade risk engine
- `common/` - Shared types and utilities

These are managed as a Cargo workspace.

### Services (Go)
Domain-specific business services in `services/`:
- `gateway/` - API gateway
- `payments/` - Payment processing
- `banking/` - Core banking
- `trading/` - Trading and OMS
- `crypto/` - Cryptocurrency integration
- `compliance/` - KYC/AML workflows

These are managed as a Go workspace.

### Infrastructure
All deployment and infrastructure code in `infra/`:
- Docker Compose for local development
- Kubernetes manifests with Kustomize overlays
- Terraform for cloud provisioning
- Ansible for bare metal

### Observability
Centralized monitoring configuration in `observability/`:
- Grafana dashboards
- Prometheus scrape configs and alerts
- OpenTelemetry collector configuration

## Consequences

### Positive
- **Atomic changes**: Changes spanning Rust core and Go services can be committed together
- **Simplified CI**: Single CI pipeline can test all components together
- **Shared tooling**: Scripts, linters, formatters can be centralized
- **Version synchronization**: No need to coordinate versions across multiple repos
- **Easy refactoring**: Moving code between modules doesn't require repo migrations
- **Consistent development environment**: One setup script works for everything

### Negative
- **Repository size**: Will grow larger over time (mitigated by Git's efficiency)
- **CI complexity**: Need to detect changed components and run targeted tests
- **Access control**: Can't easily restrict access to specific components (acceptable for open source)
- **Clone time**: Initial clone is larger (one-time cost)

### Neutral
- **Build system complexity**: Each language ecosystem (Rust/Go) maintains its own workspace
- **Deployment**: Services still deploy independently despite being in the same repo

## Alternatives Considered

### Multi-repo (Polyrepo)
One repository per service/component. Rejected because:
- Coordination overhead for cross-component changes
- Complex version management
- Difficult to maintain consistency
- Harder to onboard new contributors

### Hybrid Approach
Core in one repo, services in another. Rejected because:
- Still requires coordination for changes spanning both
- Loses benefits of true monorepo
- Added complexity with minimal benefit

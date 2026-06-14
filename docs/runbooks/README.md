# Blazil Runbooks

This directory contains operational runbooks for managing Blazil in production.

## Available Runbooks

### Deployment
- `deployment.md` - Standard deployment procedures
- `rollback.md` - Emergency rollback procedures
- `blue-green-deployment.md` - Zero-downtime deployments

### Incident Response
- `incident-response.md` - General incident response procedures
- `high-latency.md` - Debugging high latency issues
- `service-down.md` - Service recovery procedures
- `data-corruption.md` - Ledger inconsistency resolution

### Maintenance
- `scaling.md` - Horizontal and vertical scaling procedures
- `backup-restore.md` - Backup and disaster recovery
- `version-upgrade.md` - Upgrading Blazil versions
- `dependency-updates.md` - Updating third-party dependencies

### Monitoring
- `dashboard-guide.md` - Guide to Grafana dashboards
- `alert-runbook.md` - Response procedures for each alert

### AI Inference
- `clarkenai-cloud-bench.md` - Canonical Clarken 70B CPU cloud benchmark workflow
- `../architecture/002-ankatos-cortex-blazil-boundary.md` - Target-state boundary for Ankatos, Cortex v1, and Blazil

## TODO

- [x] Write deployment runbook
- [x] Write rollback procedures
- [x] Write incident response procedures
- [x] Write high-latency debugging guide
- [x] Write service-down recovery guide
- [ ] Write blue-green deployment guide
- [ ] Write data-corruption / ledger inconsistency guide
- [ ] Write scaling procedures
- [ ] Write backup/restore procedures
- [ ] Write version-upgrade guide
- [ ] Write dependency-updates guide
- [ ] Write dashboard guide
- [ ] Write per-alert response runbook

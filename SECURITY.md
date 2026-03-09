# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| main    | :white_check_mark: |
| latest  | :white_check_mark: |

## Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub issues.**

The Blazil project takes security seriously. Given that Blazil is financial infrastructure software, security vulnerabilities must be handled with care.

### How to Report

1. **Email**: Send a detailed report to the Blazil security team (security@blazil.dev — placeholder until configured)

2. **Include in your report**:
   - A description of the vulnerability and its potential impact
   - Steps to reproduce the issue
   - Any relevant proof-of-concept code (please limit to what's necessary to demonstrate the issue)
   - Your name and contact information (optional, for recognition)

### What to Expect

- **Acknowledgment**: We will acknowledge receipt of your report within 48 hours
- **Assessment**: We will assess the severity and impact within 5 business days
- **Updates**: We will keep you informed of our progress
- **Resolution**: We aim to resolve critical vulnerabilities within 30 days
- **Disclosure**: We will coordinate disclosure with you before publishing

### Security Considerations for Blazil

As a financial infrastructure platform, Blazil has heightened security requirements:

- **Transaction integrity**: Any vulnerability affecting transaction ordering, amounts, or balances is critical
- **Authentication/Authorization**: Any bypass of access controls is critical
- **Data exposure**: Any exposure of financial data or PII is critical
- **Denial of Service**: High-volume attacks that could disrupt transaction processing are serious

### Scope

The following are in scope for security reports:

- All production code in `core/` (Rust crates)
- All production code in `services/` (Go services)
- Infrastructure configurations in `infra/`
- Authentication and authorization flows

The following are out of scope:

- Development tooling and scripts
- Documentation
- Test code
- Benchmark code

### Security Practices

Blazil maintains the following security practices:

- Regular dependency audits (`cargo audit`, `govulncheck`)
- Container image scanning with Trivy
- Static analysis in CI pipeline
- Dependency pinning for critical components

## Hall of Fame

We recognize security researchers who responsibly disclose vulnerabilities:

*No entries yet — be the first!*

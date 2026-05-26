# ADR 0004 — Authentication and Authorisation Strategy

**Status:** Accepted  
**Date:** 2026-07-03  
**Deciders:** Architecture Room  

---

## Context

Blazil exposes two distinct network surfaces that require identity verification and access control:

1. **External / client-facing surface** — the gRPC Gateway (port 50050) where mobile apps, web clients, and third-party partners authenticate as end-users or service principals.
2. **Internal / service-to-service surface** — the TCP engine port (50051) and Go microservice mesh, where services call each other with no human interaction.

Evaluated options:

| Option | External | Internal | Notes |
|--------|----------|----------|-------|
| JWT only | ✓ | △ | Tokens are long-lived if not carefully managed; no transport-layer identity |
| OAuth 2.0 + OIDC only | ✓ | ✗ | Requires identity provider; adds latency; token exchange overhead for internal calls |
| mTLS only | ✗ | ✓ | No standard bearer credential; poor UX for human clients |
| JWT + mTLS (hybrid) | ✓ | ✓ | Two layers; each fits its surface naturally |

---

## Decision

Adopt a **hybrid JWT (external) + mTLS (internal)** model.

### External: Short-Lived JWTs via OIDC

- Gateway validates a signed JWT in the `Authorization: Bearer` header.
- Tokens are RS256-signed by an external OIDC provider (Keycloak / Auth0 / AWS Cognito).
- Maximum token lifetime: **15 minutes** for end-user sessions, **1 hour** for machine principals.
- Scope claims control coarse-grained permissions (`payments:write`, `ledger:read`, etc.).
- The gateway enforces claim checks before forwarding to internal services.

### Internal: mTLS with Short-Lived Certificates

- All service-to-service calls use TLS with mutual certificate authentication.
- Certificates are issued by an in-cluster CA (cert-manager + a private ClusterIssuer).
- Leaf certificates have a **24-hour** TTL and are rotated automatically by cert-manager.
- Services reject connections that do not present a valid peer certificate from the internal CA.
- The OTel collector, Prometheus scrape endpoints, and internal gRPC calls all participate.

### Secrets rotation

- JWT signing keys are rotated on a 90-day schedule via `libs/secrets/vault.go` (`RotateSecret`).
- mTLS certificates rotate automatically via cert-manager; no manual step required.
- Vault AppRole credentials used by services rotate on first use (response-wrapping tokens).

---

## Alternatives considered

### OAuth 2.0 + OIDC for internal calls

Rejected. Token introspection or JWKS fetches add ≥5 ms of round-trip latency per call.  Internal services already share the same Kubernetes namespace and trust boundary; mTLS is sufficient and zero-latency relative to application processing.

### API keys only

Rejected for the external surface. API keys are long-lived secrets that cannot encode fine-grained claims and are difficult to revoke in real time. JWT expiry provides a natural revocation window without a central revocation list.

### SPIFFE / SPIRE

Considered as a superset of mTLS for the internal mesh. Deferred to a future ADR; cert-manager satisfies the current scale. SPIFFE identities can be layered on top without changing the transport model.

---

## Consequences

### Positive

- External clients get a standard, widely-understood authentication flow with short credential windows.
- Internal service-to-service calls cannot be spoofed even if one service is compromised — the attacker must also forge a certificate.
- mTLS certificates are automatically rotated; no operator action required after initial setup.
- Scoped JWTs allow fine-grained audit logs — every external action is associated with a verified identity and scope.

### Negative / risks

- Two credential systems increase operational surface: developers must understand both JWT claims and mTLS certificate management.
- The OIDC provider becomes an availability dependency for external traffic. Mitigation: Gateway caches public JWKS keys with a 5-minute TTL so brief OIDC provider outages do not drop traffic.
- Certificate rotation windows (24h) mean a compromised internal service certificate is valid for up to 24 hours. Mitigation: cert-manager `CertificateRequest` can be revoked manually; network policies prevent lateral movement.

---

## References

- `libs/auth/` — JWT validation middleware
- `infra/k8s/base/cert-manager/cluster-issuer.yaml` — internal CA configuration
- `libs/secrets/vault.go` — `RotateSecret` implementation
- `libs/observability/tracing.go` — conditional TLS for OTel exporter
- RFC 7519 (JWT), RFC 8705 (mTLS for OAuth2 clients)

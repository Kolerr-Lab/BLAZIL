# Access Control Framework

**Document Version:** 1.0  
**Last Updated:** May 9, 2026  
**Classification:** Internal  

## Overview

Blazil implements a defense-in-depth access control strategy with multiple layers of authentication and authorization. All access is logged via the audit trail for compliance and security monitoring.

## Authentication Methods

### 1. API Key Authentication

**Use Case:** External clients, integrations, automated systems

**Implementation:**
```rust
// API key format: blazil_<environment>_<32-byte-hex>
// Example: blazil_prod_a3f9c8e2d1b4567890abcdef12345678
```

**Storage:**
- API keys hashed with SHA-256 before storage
- Salt: Per-key random 32-byte salt
- Database schema:
  ```sql
  CREATE TABLE api_keys (
      key_id UUID PRIMARY KEY,
      key_hash BYTEA NOT NULL,
      key_salt BYTEA NOT NULL,
      account_id UUID NOT NULL,
      permissions TEXT[] NOT NULL,
      created_at TIMESTAMPTZ NOT NULL,
      expires_at TIMESTAMPTZ,
      last_used_at TIMESTAMPTZ,
      revoked_at TIMESTAMPTZ
  );
  ```

**Rotation:**
- Manual rotation by account owner
- Automatic expiration after 1 year
- Revocation: Immediate, logged to audit trail

**Usage:**
```bash
curl -H "Authorization: Bearer blazil_prod_xyz123" \
     https://api.blazil.io/v1/transactions
```

### 2. JWT Token Authentication

**Use Case:** User sessions, web applications

**Token Structure:**
```json
{
  "iss": "blazil-auth",
  "sub": "user_alice",
  "aud": "blazil-api",
  "exp": 1715259600,
  "iat": 1715256000,
  "permissions": ["read:transactions", "write:transactions"],
  "account_id": "acc_123"
}
```

**Signing:**
- Algorithm: RS256 (RSA with SHA-256)
- Key size: 4096 bits
- Key rotation: Every 90 days
- Public keys published at `https://api.blazil.io/.well-known/jwks.json`

**Expiration:**
- Access token: 1 hour
- Refresh token: 7 days
- Refresh token rotation on use

**Verification:**
```rust
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};

let token_data = decode::<Claims>(
    &token,
    &DecodingKey::from_rsa_pem(public_key)?,
    &Validation::new(Algorithm::RS256),
)?;
```

### 3. Mutual TLS (mTLS)

**Use Case:** Service-to-service communication

**Certificate Structure:**
- Subject: `CN=service-name.blazil.internal`
- Issuer: Blazil Internal CA
- Validity: 90 days
- Key usage: Digital signature, Key encipherment
- Extended key usage: Client authentication, Server authentication

**Certificate Issuance:**
```bash
# Generate CSR
openssl req -new -newkey rsa:4096 -nodes \
  -keyout service.key \
  -out service.csr \
  -subj "/CN=engine.blazil.internal"

# Sign with internal CA
openssl ca -config ca.conf \
  -in service.csr \
  -out service.crt \
  -days 90
```

**Verification:**
```rust
let connector = HttpsConnector::builder()
    .with_tls_config(
        ClientConfig::builder()
            .with_root_certificates(ca_certs)
            .with_client_auth_cert(client_cert, client_key)?
            .build()
    )
    .https_only()
    .enable_http2()
    .build();
```

### 4. Admin Multi-Factor Authentication (MFA)

**Use Case:** Administrative access, sensitive operations

**Implementation:**
- Primary: Password (PBKDF2-SHA256, 100k iterations)
- Secondary: TOTP (Time-based One-Time Password, RFC 6238)

**TOTP Configuration:**
- Algorithm: SHA-256
- Digits: 6
- Period: 30 seconds
- Window: ±1 period (allows for clock skew)

**Backup Codes:**
- 10 single-use backup codes generated at enrollment
- Stored hashed (SHA-256)
- User must acknowledge receipt

**MFA Enforcement:**
```rust
// Require MFA for admin operations
if user.role == Role::Admin && !session.mfa_verified {
    return Err(AuthError::MfaRequired);
}
```

## Authorization Model

### Role-Based Access Control (RBAC)

**Roles:**

| Role | Description | Permissions |
|------|-------------|-------------|
| `viewer` | Read-only access | `read:transactions`, `read:accounts` |
| `operator` | Transaction submission | `read:*`, `write:transactions` |
| `admin` | Full administrative access | `*:*` |
| `compliance` | Compliance operations | `read:*`, `write:compliance`, `read:audit_logs` |
| `service` | Service account | Scoped to specific resources |

**Permission Format:**
```
<action>:<resource>:<scope>

Examples:
- read:transactions:account_123
- write:transactions:*
- admin:config:region_sg
```

### Resource-Based Access Control

**Account-Level Scoping:**
```rust
// User can only access transactions for their own account
let txs = db.query(
    "SELECT * FROM transactions WHERE account_id = $1",
    &[user.account_id]
)?;
```

**Transaction-Level Scoping:**
```rust
// Verify user has access to specific transaction
if !user.has_permission(&format!("read:transactions:{}", tx.account_id)) {
    return Err(AuthError::Forbidden);
}
```

### Attribute-Based Access Control (ABAC)

**Context-Aware Authorization:**
```rust
// Example: Restrict high-value transactions to specific IPs
if tx.amount > 100_000_00 && !allowed_ips.contains(&request.ip) {
    log.record(AuditEvent::new(
        tx.id.to_string(),
        user.id.clone(),
        AuditAction::AccessControlCheck,
    ).with_error("IP not in allow-list for high-value transactions"));
    
    return Err(AuthError::IpRestricted);
}
```

## Access Control Enforcement Points

### 1. API Gateway

**Location:** Edge of the system, before any business logic

**Checks:**
- Authentication: Verify API key or JWT
- Rate limiting: Enforce per-key/per-user limits
- IP allowlist: Check source IP against allowlist
- Audit logging: Record all authentication attempts

```rust
async fn authenticate_request(req: &Request) -> Result<User, AuthError> {
    let auth_header = req.headers().get("Authorization")
        .ok_or(AuthError::MissingAuth)?;
    
    let token = parse_bearer_token(auth_header)?;
    let user = verify_token(&token).await?;
    
    audit_log.record(AuditEvent::new(
        req.id.to_string(),
        user.id.clone(),
        AuditAction::ApiAuthentication,
    ).with_result("success")).await;
    
    Ok(user)
}
```

### 2. Engine Layer

**Location:** Before transaction processing

**Checks:**
- Authorization: Verify user has `write:transactions` permission
- Account ownership: Verify user owns source account
- Business rules: Enforce account limits, transaction types

```rust
async fn check_transaction_permission(
    user: &User,
    tx: &TransactionRequest,
) -> Result<(), AuthError> {
    // Check basic permission
    if !user.has_permission("write:transactions") {
        return Err(AuthError::Forbidden);
    }
    
    // Check account ownership
    if tx.source_account != user.account_id {
        return Err(AuthError::AccountMismatch);
    }
    
    audit_log.record(AuditEvent::new(
        tx.id.to_string(),
        user.id.clone(),
        AuditAction::AccessControlCheck,
    ).with_result("success")).await;
    
    Ok(())
}
```

### 3. Ledger Layer

**Location:** Before ledger commit

**Checks:**
- Service authentication: Verify caller is authorized service
- Idempotency: Prevent duplicate transactions
- Balance checks: Enforced by TigerBeetle

```rust
async fn submit_to_ledger(
    caller: &ServiceIdentity,
    tx: &Transfer,
) -> Result<(), LedgerError> {
    // Verify caller is authorized
    if !caller.has_permission("write:ledger") {
        return Err(LedgerError::Unauthorized);
    }
    
    // Submit to TigerBeetle
    let result = ledger_client.create_transfers(vec![tx]).await?;
    
    audit_log.record(AuditEvent::new(
        tx.id.to_string(),
        caller.id.clone(),
        AuditAction::LedgerSubmitted,
    ).with_result(if result.is_ok() { "success" } else { "failure" })).await;
    
    result
}
```

## Audit Logging

**All access control events are logged:**

```rust
// Authentication success
audit_log.record(AuditEvent::new(
    request_id,
    api_key,
    AuditAction::ApiAuthentication,
).with_result("success").with_metadata(serde_json::json!({
    "ip_address": "192.168.1.100",
    "user_agent": "blazil-sdk/1.0",
    "endpoint": "/api/v1/transactions"
})));

// Authentication failure
audit_log.record(AuditEvent::new(
    request_id,
    "unknown",
    AuditAction::ApiAuthentication,
).with_error("Invalid API key").with_metadata(serde_json::json!({
    "ip_address": "10.0.0.50",
    "attempted_key": "blazil_prod_invalid..."
})));

// Authorization check
audit_log.record(AuditEvent::new(
    transaction_id,
    user_id,
    AuditAction::AccessControlCheck,
).with_result("success").with_metadata(serde_json::json!({
    "resource": "transactions",
    "action": "write",
    "account_id": "acc_123"
})));
```

## Monitoring & Alerting

### Failed Authentication Attempts

**Alert:** > 5 failed attempts from same IP in 5 minutes

```promql
rate(audit_authentication_failure_total{ip="x.x.x.x"}[5m]) > 5
```

### Unauthorized Access Attempts

**Alert:** Any `AccessControlCheck` with result=failure

```promql
audit_access_control_failure_total > 0
```

### Anomalous Access Patterns

**Alert:** Access from new IP for existing user

```rust
if !user.known_ips.contains(&request.ip) {
    notify_security_team(format!(
        "User {} accessed from new IP: {}",
        user.id, request.ip
    ));
}
```

## Compliance Mapping

### SOC 2 Type II

✅ **CC6.1:** Logical access is restricted  
✅ **CC6.2:** Authentication mechanisms are implemented  
✅ **CC6.3:** Authorization mechanisms are implemented  
✅ **CC6.6:** Access is reviewed periodically  
✅ **CC7.2:** Access is logged and monitored  

### PCI DSS

✅ **Requirement 7:** Restrict access by business need-to-know  
✅ **Requirement 8:** Identify and authenticate access  
✅ **Requirement 10:** Track and monitor all access  

## Access Review Process

**Frequency:** Quarterly

**Procedure:**
1. Export list of all API keys and users
2. Review permissions for each user/key
3. Revoke unused keys (no activity in 90 days)
4. Verify MFA enrollment for all admin users
5. Document review in audit log

**Automation:**
```bash
# Generate access review report
cargo run --bin access-review -- --output quarterly-review-2026-Q2.json
```

---

**Maintained by:** Kolerr Lab Security Team  
**Review Cycle:** Quarterly  
**Next Review:** August 9, 2026

# Encryption at Rest and In Transit

**Document Version:** 1.0  
**Last Updated:** May 9, 2026  
**Classification:** Public  

## Overview

Blazil implements defense-in-depth encryption for all sensitive data, covering both data at rest and data in transit. This document describes the encryption methods, key management practices, and compliance posture.

## Encryption at Rest

### TigerBeetle Ledger Data

**Method:** Block-level encryption via LUKS2/dm-crypt (Linux) or AWS EBS encryption

**Implementation:**
```bash
# Linux setup with LUKS2
cryptsetup luksFormat --type luks2 --cipher aes-xts-plain64 --key-size 512 /dev/nvme0n1
cryptsetup open /dev/nvme0n1 tigerbeetle_data
mkfs.ext4 /dev/mapper/tigerbeetle_data
mount /dev/mapper/tigerbeetle_data /var/lib/tigerbeetle
```

**AWS EBS:**
- AES-256 encryption enabled by default on all i4i.4xlarge instance volumes
- KMS-managed encryption keys with automatic rotation
- Encrypted snapshots for backups

**Key Management:**
- Keys stored in AWS KMS or HashiCorp Vault
- Automatic key rotation every 90 days
- Keys never stored in plaintext on disk

### Audit Logs

**Method:** Volume-level encryption (same as TigerBeetle data)

**Storage:**
- Audit logs written to encrypted volumes
- Hash chain provides tamper detection (not encryption)
- Export format (JSON/CEF) should be encrypted before transfer

### Configuration Files

**Method:** Application-level encryption using `age` or `sops`

**Example:**
```bash
# Encrypt configuration with age
age -e -o config.enc.toml -i key.age config.toml

# Decrypt at runtime
age -d -i key.age config.enc.toml > config.toml
```

**Key Management:**
- Configuration keys stored in secure vault (AWS Secrets Manager, Vault)
- Never committed to Git in plaintext
- Separate keys per environment (dev, staging, prod)

### Application Secrets

**Method:** Secret management systems (AWS Secrets Manager, HashiCorp Vault)

**Implementation:**
```rust
// Example: Fetch database credentials at runtime
let db_password = vault_client
    .get_secret("blazil/prod/tigerbeetle/password")
    .await?;
```

**Rotation:**
- Automatic rotation every 30-90 days
- Zero-downtime rotation support
- Old secrets retained for 7 days for rollback

## Encryption in Transit

### Aeron IPC (Internal)

**Method:** Shared memory, no network exposure

**Security:**
- Unix file permissions: `0600` (owner read/write only)
- No encryption needed (local-only, process isolation)
- Protected by OS memory isolation

**Use Case:**
- Engine → Dataloader
- Engine → Inference service
- High-throughput, zero-copy transport

### TigerBeetle VSR Cluster

**Method:** TLS 1.3 with mutual authentication

**Configuration:**
```toml
[tigerbeetle.cluster]
tls_enabled = true
tls_version = "1.3"
cipher_suites = ["TLS_AES_256_GCM_SHA384", "TLS_CHACHA20_POLY1305_SHA256"]
mutual_tls = true
cert_path = "/etc/tigerbeetle/certs/node.crt"
key_path = "/etc/tigerbeetle/certs/node.key"
ca_path = "/etc/tigerbeetle/certs/ca.crt"
```

**Certificate Management:**
- Certificates issued by internal CA
- 90-day validity period with automatic renewal
- Certificate rotation without cluster downtime

### API Endpoints (Future)

**Method:** HTTPS with TLS 1.3 only

**Configuration:**
```rust
// Enforce TLS 1.3 only
HttpServer::new(|| App::new())
    .bind_openssl("0.0.0.0:443", {
        let mut builder = SslAcceptor::mozilla_modern(SslMethod::tls())?;
        builder.set_min_proto_version(Some(SslVersion::TLS1_3))?;
        builder
    })?
    .run()
    .await
```

**Cipher Suites:**
- `TLS_AES_256_GCM_SHA384` (primary)
- `TLS_CHACHA20_POLY1305_SHA256` (mobile/edge)

### Service-to-Service Communication

**Method:** Mutual TLS (mTLS) with certificate-based authentication

**Implementation:**
- Each service has unique X.509 certificate
- Certificate CN = service identifier
- Service mesh: Istio/Linkerd with automatic mTLS injection

**Example:**
```rust
// Mutual TLS client configuration
let client = reqwest::Client::builder()
    .use_rustls_tls()
    .add_root_certificate(ca_cert)
    .identity(client_cert_and_key)
    .build()?;
```

## Key Management

### Key Hierarchy

```
Root Key (HSM-protected)
  ├── TigerBeetle Data Encryption Key (DEK)
  ├── Audit Log Encryption Key
  ├── Configuration Encryption Key
  └── TLS Certificate Signing Key
```

### Key Storage

**Production:**
- AWS KMS for cloud deployments
- HashiCorp Vault for on-premises
- Hardware Security Module (HSM) for root keys

**Development:**
- Age-encrypted files
- Local Vault instance (never production keys)

### Key Rotation Schedule

| Key Type | Rotation Period | Method |
|----------|----------------|--------|
| Data Encryption Keys | 90 days | Automatic via KMS |
| TLS Certificates | 90 days | Cert-manager auto-renewal |
| API Keys | On-demand | Manual rotation + audit log |
| Service Credentials | 30 days | Automatic via Vault |

## Compliance Standards

### SOC 2 Type II

✅ **CC6.7:** Data is encrypted at rest  
✅ **CC6.7:** Data is encrypted in transit  
✅ **CC6.6:** Encryption keys are managed securely  
✅ **CC6.1:** Logical access to encryption keys is restricted  

### PCI DSS (Future)

✅ **Requirement 3.4:** Cryptography protects cardholder data  
✅ **Requirement 4.1:** Strong cryptography for transmission  
✅ **Requirement 3.5:** Cryptographic keys are protected  

### GDPR

✅ **Article 32:** Encryption of personal data  
✅ **Article 25:** Data protection by design  

## Verification

### At-Rest Encryption

```bash
# Verify LUKS encryption
cryptsetup status tigerbeetle_data

# Verify EBS encryption (AWS)
aws ec2 describe-volumes --volume-ids vol-xxxxx --query 'Volumes[0].Encrypted'
```

### In-Transit Encryption

```bash
# Verify TLS version
openssl s_client -connect node1.blazil.internal:3000 -tls1_3

# Check cipher suite
nmap --script ssl-enum-ciphers -p 3000 node1.blazil.internal
```

### Certificate Expiry Monitoring

```bash
# Check certificate expiration
openssl x509 -in /etc/tigerbeetle/certs/node.crt -noout -enddate

# Automated monitoring via Prometheus
tigerbeetle_tls_cert_expiry_days{node="node1"} 45
```

## Incident Response

### Suspected Key Compromise

1. **Immediate:** Rotate compromised key via KMS/Vault
2. **Audit:** Check access logs for unauthorized usage
3. **Re-encrypt:** Re-encrypt data with new key (if DEK compromised)
4. **Document:** Record incident in audit log

### Certificate Compromise

1. **Revoke:** Add certificate to CRL immediately
2. **Reissue:** Issue new certificate with new key pair
3. **Update:** Deploy new certificate to all nodes
4. **Monitor:** Alert on any use of revoked certificate

## Audit Trail

All encryption-related events are logged via `blazil-audit`:

```rust
log.record(AuditEvent::new(
    "config_001".to_string(),
    "admin_user".to_string(),
    AuditAction::ConfigurationChanged,
).with_metadata(serde_json::json!({
    "action": "key_rotation",
    "key_type": "tigerbeetle_dek",
    "old_key_id": "key-2026-02-01",
    "new_key_id": "key-2026-05-09"
})));
```

---

**Maintained by:** Kolerr Lab Security Team  
**Review Cycle:** Quarterly  
**Next Review:** August 9, 2026

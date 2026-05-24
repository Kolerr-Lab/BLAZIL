// Package tenant provides the data model and persistence layer for Blazil Cloud tenants.
//
// Security notes:
//   - API keys are generated with crypto/rand (256-bit entropy).
//   - Only the SHA-256 hash of a key is stored; the raw key is returned once at
//     creation and never persisted.
//   - All SQL queries use parameterised placeholders — no string concatenation.
//   - last_used_at is updated asynchronously (fire-and-forget) to keep the hot
//     path latency unaffected by the write.
package tenant

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// Sentinel errors returned by Store methods.
var (
	ErrNotFound   = errors.New("tenant: not found")
	ErrKeyRevoked = errors.New("tenant: api key revoked")
	ErrSuspended  = errors.New("tenant: account suspended")
)

// Tier identifies a billing tier. Mirrors metering.Tier.
type Tier = string

const (
	TierFree       Tier = "free"
	TierCloudSaaS  Tier = "cloud_saas"
	TierEnterprise Tier = "enterprise"
)

// Tenant holds the metadata for one Blazil Cloud customer.
type Tenant struct {
	ID             string
	Name           string
	Email          string
	Tier           Tier
	RateLimitRPS   int
	RateLimitBurst int
	CreatedAt      time.Time
	SuspendedAt    *time.Time
}

// APIKey is a non-secret view of a tenant API key.
// RawKey is only populated immediately after IssueAPIKey(); never stored.
type APIKey struct {
	ID         string
	TenantID   string
	Prefix     string // first 16 chars of raw key, safe for dashboards
	Name       string
	CreatedAt  time.Time
	LastUsedAt *time.Time
	RevokedAt  *time.Time
	RawKey     string // present only at creation — json:"-" to prevent accidental logging
}

// Store is the tenant persistence interface.
// All operations are context-aware and propagate cancellation.
type Store interface {
	// CreateTenant inserts a new tenant and returns the created record.
	CreateTenant(ctx context.Context, name, email string, tier Tier,
		rateLimitRPS, rateLimitBurst int) (*Tenant, error)
	// GetTenant returns a tenant by ID or ErrNotFound.
	GetTenant(ctx context.Context, id string) (*Tenant, error)
	// ListTenants returns all tenants ordered by created_at DESC.
	ListTenants(ctx context.Context) ([]*Tenant, error)
	// SuspendTenant marks a tenant as suspended. All API keys stop working.
	SuspendTenant(ctx context.Context, id string) error
	// UnsuspendTenant removes the suspension flag.
	UnsuspendTenant(ctx context.Context, id string) error

	// IssueAPIKey generates and stores a new API key for the given tenant.
	// The returned APIKey.RawKey contains the full key — the only time it is available.
	IssueAPIKey(ctx context.Context, tenantID, keyName string) (*APIKey, error)
	// LookupAPIKey validates rawKey and returns the associated Tenant and APIKey.
	// Returns ErrNotFound, ErrKeyRevoked, or ErrSuspended on failure.
	LookupAPIKey(ctx context.Context, rawKey string) (*Tenant, *APIKey, error)
	// RevokeAPIKey marks a key as revoked. Returns ErrNotFound if already revoked.
	RevokeAPIKey(ctx context.Context, keyID string) error
	// ListAPIKeys returns all keys for a tenant (including revoked), newest first.
	ListAPIKeys(ctx context.Context, tenantID string) ([]*APIKey, error)
	// Ping verifies the database connection is healthy. Used by the readiness probe.
	Ping(ctx context.Context) error
}

// pgStore is the Postgres-backed implementation of Store.
type pgStore struct {
	db *pgxpool.Pool
}

// NewPGStore creates a Store backed by the given pgxpool connection pool.
func NewPGStore(db *pgxpool.Pool) Store {
	return &pgStore{db: db}
}

// ── CreateTenant ─────────────────────────────────────────────────────────────

func (s *pgStore) CreateTenant(ctx context.Context, name, email string,
	tier Tier, rateLimitRPS, rateLimitBurst int,
) (*Tenant, error) {
	id := uuid.New().String()
	var t Tenant
	err := s.db.QueryRow(ctx, `
		INSERT INTO tenants (id, name, email, tier, rate_limit_rps, rate_limit_burst)
		VALUES ($1, $2, $3, $4, $5, $6)
		RETURNING id, name, email, tier, rate_limit_rps, rate_limit_burst, created_at, suspended_at`,
		id, name, email, tier, rateLimitRPS, rateLimitBurst,
	).Scan(&t.ID, &t.Name, &t.Email, &t.Tier,
		&t.RateLimitRPS, &t.RateLimitBurst, &t.CreatedAt, &t.SuspendedAt)
	if err != nil {
		return nil, fmt.Errorf("create tenant: %w", err)
	}
	return &t, nil
}

// ── GetTenant ────────────────────────────────────────────────────────────────

func (s *pgStore) GetTenant(ctx context.Context, id string) (*Tenant, error) {
	var t Tenant
	err := s.db.QueryRow(ctx, `
		SELECT id, name, email, tier, rate_limit_rps, rate_limit_burst, created_at, suspended_at
		FROM tenants WHERE id = $1`, id,
	).Scan(&t.ID, &t.Name, &t.Email, &t.Tier,
		&t.RateLimitRPS, &t.RateLimitBurst, &t.CreatedAt, &t.SuspendedAt)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("get tenant: %w", err)
	}
	return &t, nil
}

// ── ListTenants ──────────────────────────────────────────────────────────────

func (s *pgStore) ListTenants(ctx context.Context) ([]*Tenant, error) {
	rows, err := s.db.Query(ctx, `
		SELECT id, name, email, tier, rate_limit_rps, rate_limit_burst, created_at, suspended_at
		FROM tenants ORDER BY created_at DESC`)
	if err != nil {
		return nil, fmt.Errorf("list tenants: %w", err)
	}
	defer rows.Close()
	var tenants []*Tenant
	for rows.Next() {
		var t Tenant
		if err := rows.Scan(&t.ID, &t.Name, &t.Email, &t.Tier,
			&t.RateLimitRPS, &t.RateLimitBurst, &t.CreatedAt, &t.SuspendedAt); err != nil {
			return nil, fmt.Errorf("scan tenant: %w", err)
		}
		tenants = append(tenants, &t)
	}
	return tenants, rows.Err()
}

// ── SuspendTenant / UnsuspendTenant ──────────────────────────────────────────

func (s *pgStore) SuspendTenant(ctx context.Context, id string) error {
	tag, err := s.db.Exec(ctx,
		`UPDATE tenants SET suspended_at = now() WHERE id = $1 AND suspended_at IS NULL`, id)
	if err != nil {
		return fmt.Errorf("suspend tenant: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

func (s *pgStore) UnsuspendTenant(ctx context.Context, id string) error {
	_, err := s.db.Exec(ctx, `UPDATE tenants SET suspended_at = NULL WHERE id = $1`, id)
	return err
}

// ── IssueAPIKey ──────────────────────────────────────────────────────────────

func (s *pgStore) IssueAPIKey(ctx context.Context, tenantID, keyName string) (*APIKey, error) {
	rawKey, keyHash, prefix, err := GenerateAPIKey()
	if err != nil {
		return nil, err
	}
	id := uuid.New().String()
	var key APIKey
	err = s.db.QueryRow(ctx, `
		INSERT INTO api_keys (id, tenant_id, key_hash, prefix, name)
		VALUES ($1, $2, $3, $4, $5)
		RETURNING id, tenant_id, prefix, name, created_at`,
		id, tenantID, keyHash, prefix, keyName,
	).Scan(&key.ID, &key.TenantID, &key.Prefix, &key.Name, &key.CreatedAt)
	if err != nil {
		return nil, fmt.Errorf("issue api key: %w", err)
	}
	// RawKey is the only opportunity to retrieve the key — return it once.
	key.RawKey = rawKey
	return &key, nil
}

// ── LookupAPIKey ─────────────────────────────────────────────────────────────

func (s *pgStore) LookupAPIKey(ctx context.Context, rawKey string) (*Tenant, *APIKey, error) {
	keyHash := HashAPIKey(rawKey)
	var key APIKey
	var tenant Tenant
	err := s.db.QueryRow(ctx, `
		SELECT
			k.id, k.tenant_id, k.prefix, k.name, k.created_at, k.last_used_at, k.revoked_at,
			t.id, t.name, t.email, t.tier,
			t.rate_limit_rps, t.rate_limit_burst, t.created_at, t.suspended_at
		FROM api_keys k
		JOIN tenants t ON t.id = k.tenant_id
		WHERE k.key_hash = $1`, keyHash,
	).Scan(
		&key.ID, &key.TenantID, &key.Prefix, &key.Name,
		&key.CreatedAt, &key.LastUsedAt, &key.RevokedAt,
		&tenant.ID, &tenant.Name, &tenant.Email, &tenant.Tier,
		&tenant.RateLimitRPS, &tenant.RateLimitBurst, &tenant.CreatedAt, &tenant.SuspendedAt,
	)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil, ErrNotFound
	}
	if err != nil {
		return nil, nil, fmt.Errorf("lookup api key: %w", err)
	}
	if key.RevokedAt != nil {
		return nil, nil, ErrKeyRevoked
	}
	if tenant.SuspendedAt != nil {
		return nil, nil, ErrSuspended
	}

	// Update last_used_at asynchronously — don't add DB latency to the hot path.
	keyID := key.ID
	go func() {
		updateCtx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		defer cancel()
		_, _ = s.db.Exec(updateCtx,
			`UPDATE api_keys SET last_used_at = now() WHERE id = $1`, keyID)
	}()

	return &tenant, &key, nil
}

// ── RevokeAPIKey ─────────────────────────────────────────────────────────────

func (s *pgStore) RevokeAPIKey(ctx context.Context, keyID string) error {
	tag, err := s.db.Exec(ctx,
		`UPDATE api_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`, keyID)
	if err != nil {
		return fmt.Errorf("revoke api key: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

// ── ListAPIKeys ───────────────────────────────────────────────────────────────

func (s *pgStore) ListAPIKeys(ctx context.Context, tenantID string) ([]*APIKey, error) {
	rows, err := s.db.Query(ctx, `
		SELECT id, tenant_id, prefix, name, created_at, last_used_at, revoked_at
		FROM api_keys
		WHERE tenant_id = $1
		ORDER BY created_at DESC`, tenantID)
	if err != nil {
		return nil, fmt.Errorf("list api keys: %w", err)
	}
	defer rows.Close()
	var keys []*APIKey
	for rows.Next() {
		var k APIKey
		if err := rows.Scan(&k.ID, &k.TenantID, &k.Prefix, &k.Name,
			&k.CreatedAt, &k.LastUsedAt, &k.RevokedAt); err != nil {
			return nil, fmt.Errorf("scan api key: %w", err)
		}
		keys = append(keys, &k)
	}
	return keys, rows.Err()
}

// ── Ping ──────────────────────────────────────────────────────────────────────

func (s *pgStore) Ping(ctx context.Context) error {
	return s.db.Ping(ctx)
}

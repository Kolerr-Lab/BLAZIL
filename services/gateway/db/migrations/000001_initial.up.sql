-- Blazil Gateway — initial schema
-- Run with: psql $DATABASE_URL -f 001_initial.sql
--
-- Idempotent: all statements use IF NOT EXISTS / DO NOTHING.

BEGIN;

-- Enable pgcrypto for gen_random_uuid() (Postgres 13+ has it built-in via gen_random_uuid).
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ── tenants ──────────────────────────────────────────────────────────────────
-- One row per Blazil Cloud customer.
CREATE TABLE IF NOT EXISTS tenants (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name             TEXT        NOT NULL,
    email            TEXT        NOT NULL,
    tier             TEXT        NOT NULL DEFAULT 'cloud_saas'
                                 CHECK (tier IN ('free', 'cloud_saas', 'enterprise')),
    rate_limit_rps   INTEGER     NOT NULL DEFAULT 100 CHECK (rate_limit_rps > 0),
    rate_limit_burst INTEGER     NOT NULL DEFAULT 200 CHECK (rate_limit_burst > 0),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    suspended_at     TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS tenants_email_idx ON tenants (email);

-- ── api_keys ─────────────────────────────────────────────────────────────────
-- Each tenant may have multiple named API keys.
-- The raw key is NEVER stored; only its SHA-256 hash is persisted.
CREATE TABLE IF NOT EXISTS api_keys (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    -- SHA-256(raw_key) as lowercase hex.  Used for O(1) lookup on every request.
    key_hash    TEXT        NOT NULL,
    -- First 16 characters of the raw key; used only for display in dashboards.
    prefix      TEXT        NOT NULL,
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS api_keys_hash_idx  ON api_keys (key_hash);
CREATE        INDEX IF NOT EXISTS api_keys_tenant_idx ON api_keys (tenant_id);

-- ── usage ─────────────────────────────────────────────────────────────────────
-- Metering flush target.  Each row represents one 60-second window for one tenant.
-- tx_count is accumulated (ON CONFLICT DO UPDATE) so multiple gateway instances
-- can flush independently without losing data.
CREATE TABLE IF NOT EXISTS usage (
    tenant_id    UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    window_start TIMESTAMPTZ NOT NULL,
    window_end   TIMESTAMPTZ NOT NULL,
    tx_count     BIGINT      NOT NULL DEFAULT 0 CHECK (tx_count >= 0),
    PRIMARY KEY (tenant_id, window_start)
);

CREATE INDEX IF NOT EXISTS usage_tenant_window_idx ON usage (tenant_id, window_start);

COMMIT;

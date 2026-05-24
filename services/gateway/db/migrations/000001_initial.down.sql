-- Blazil Gateway — rollback initial schema
BEGIN;
DROP INDEX IF EXISTS usage_tenant_window_idx;
DROP TABLE IF EXISTS usage;
DROP INDEX IF EXISTS api_keys_tenant_idx;
DROP INDEX IF EXISTS api_keys_hash_idx;
DROP TABLE IF EXISTS api_keys;
DROP INDEX IF EXISTS tenants_email_idx;
DROP TABLE IF EXISTS tenants;
COMMIT;

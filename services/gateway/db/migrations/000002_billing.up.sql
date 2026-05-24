-- Blazil Gateway — billing schema
-- Sprint 2: Stripe invoice persistence
--
-- Idempotent: all statements use IF NOT EXISTS / ADD COLUMN IF NOT EXISTS.

BEGIN;

-- ── Stripe customer link ─────────────────────────────────────────────────────
-- One Stripe customer per Blazil tenant. NULL until CreateCustomer is called.
ALTER TABLE tenants
    ADD COLUMN IF NOT EXISTS stripe_customer_id TEXT;

-- Partial unique index: enforces uniqueness only for non-null values.
-- Allows multiple tenants with NULL (not yet provisioned).
CREATE UNIQUE INDEX IF NOT EXISTS tenants_stripe_customer_idx
    ON tenants (stripe_customer_id)
    WHERE stripe_customer_id IS NOT NULL;

-- ── invoices ─────────────────────────────────────────────────────────────────
-- One row per billing period per tenant.  status follows Stripe lifecycle:
--   draft → open → paid
--              └─→ void  (manual cancellation)
--
-- total_micro_usd mirrors the sum of line items at generation time.
-- stripe_invoice_id is set once the invoice is pushed to Stripe.
CREATE TABLE IF NOT EXISTS invoices (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID        NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    stripe_invoice_id  TEXT,
    period_start       TIMESTAMPTZ NOT NULL,
    period_end         TIMESTAMPTZ NOT NULL,
    total_micro_usd    BIGINT      NOT NULL DEFAULT 0 CHECK (total_micro_usd >= 0),
    status             TEXT        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'open', 'paid', 'void')),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    paid_at            TIMESTAMPTZ,
    -- One invoice per tenant per period — prevents double-billing.
    UNIQUE (tenant_id, period_start)
);

CREATE INDEX IF NOT EXISTS invoices_tenant_idx ON invoices (tenant_id, period_start DESC);
CREATE UNIQUE INDEX IF NOT EXISTS invoices_stripe_idx
    ON invoices (stripe_invoice_id)
    WHERE stripe_invoice_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS invoices_status_idx ON invoices (status);

-- ── invoice_line_items ───────────────────────────────────────────────────────
-- One row per metering window within an invoice.
-- Stores the per-unit price at generation time so invoice history is immutable
-- even if pricing tiers change later.
CREATE TABLE IF NOT EXISTS invoice_line_items (
    id                   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id           UUID        NOT NULL REFERENCES invoices (id) ON DELETE CASCADE,
    window_start         TIMESTAMPTZ NOT NULL,
    tx_count             BIGINT      NOT NULL DEFAULT 0 CHECK (tx_count >= 0),
    price_per_tx_micro   BIGINT      NOT NULL DEFAULT 0 CHECK (price_per_tx_micro >= 0),
    total_micro_usd      BIGINT      NOT NULL DEFAULT 0 CHECK (total_micro_usd >= 0)
);

CREATE INDEX IF NOT EXISTS invoice_items_invoice_idx
    ON invoice_line_items (invoice_id, window_start ASC);

COMMIT;

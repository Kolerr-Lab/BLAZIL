-- Blazil Gateway — rollback billing schema
BEGIN;
DROP INDEX IF EXISTS invoice_items_invoice_idx;
DROP TABLE IF EXISTS invoice_line_items;
DROP INDEX IF EXISTS invoices_status_idx;
DROP INDEX IF EXISTS invoices_stripe_idx;
DROP INDEX IF EXISTS invoices_tenant_idx;
DROP TABLE IF EXISTS invoices;
DROP INDEX IF EXISTS tenants_stripe_customer_idx;
ALTER TABLE tenants DROP COLUMN IF EXISTS stripe_customer_id;
COMMIT;

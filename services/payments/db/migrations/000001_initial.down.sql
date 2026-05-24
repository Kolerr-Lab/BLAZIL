DROP INDEX IF EXISTS payments_credit_account_idx;
DROP INDEX IF EXISTS payments_debit_account_idx;
DROP INDEX IF EXISTS payments_status_created_idx;
DROP INDEX IF EXISTS payments_idempotency_key_idx;
DROP TABLE IF EXISTS payments;

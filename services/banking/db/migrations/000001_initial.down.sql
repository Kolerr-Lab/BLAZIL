-- Blazil Banking — rollback initial schema
BEGIN;
DROP INDEX IF EXISTS transactions_account_idx;
DROP TABLE IF EXISTS transactions;
DROP TABLE IF EXISTS balances;
DROP INDEX IF EXISTS accounts_owner_idx;
DROP TABLE IF EXISTS accounts;
COMMIT;

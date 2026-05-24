-- Blazil Crypto — rollback initial schema
BEGIN;
DROP INDEX IF EXISTS withdrawals_status_idx;
DROP INDEX IF EXISTS withdrawals_wallet_idx;
DROP TABLE IF EXISTS withdrawals;
DROP INDEX IF EXISTS deposits_status_idx;
DROP INDEX IF EXISTS deposits_wallet_idx;
DROP INDEX IF EXISTS deposits_txhash_idx;
DROP TABLE IF EXISTS deposits;
DROP INDEX IF EXISTS wallets_address_idx;
DROP INDEX IF EXISTS wallets_owner_idx;
DROP TABLE IF EXISTS wallets;
COMMIT;

-- Blazil Trading — rollback initial schema
BEGIN;
DROP INDEX IF EXISTS trades_instr_idx;
DROP INDEX IF EXISTS trades_taker_idx;
DROP INDEX IF EXISTS trades_maker_idx;
DROP TABLE IF EXISTS trades;
DROP INDEX IF EXISTS orders_status_idx;
DROP INDEX IF EXISTS orders_instrument_idx;
DROP INDEX IF EXISTS orders_owner_idx;
DROP TABLE IF EXISTS orders;
COMMIT;

-- Blazil Crypto — initial schema
-- Run with: psql $CRYPTO_DATABASE_URL -f 001_initial.sql
-- Idempotent: all statements use IF NOT EXISTS.

BEGIN;

-- ── wallets ────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS wallets (
    id         TEXT    PRIMARY KEY,
    owner_id   TEXT    NOT NULL,
    chain_id   INTEGER NOT NULL,
    address    TEXT    NOT NULL,
    type       TEXT    NOT NULL DEFAULT 'deposit',
    status     TEXT    NOT NULL DEFAULT 'active'
);

CREATE INDEX IF NOT EXISTS wallets_owner_idx   ON wallets (owner_id);
CREATE INDEX IF NOT EXISTS wallets_address_idx ON wallets (address);

-- ── deposits ──────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS deposits (
    id                  TEXT        PRIMARY KEY,
    wallet_id           TEXT        NOT NULL REFERENCES wallets (id),
    account_id          TEXT        NOT NULL,
    tx_hash             TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    amount_minor_units  BIGINT      NOT NULL CHECK (amount_minor_units > 0),
    status              TEXT        NOT NULL DEFAULT 'detected',
    confirmations       INTEGER     NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at        TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS deposits_txhash_idx ON deposits (tx_hash);
CREATE        INDEX IF NOT EXISTS deposits_wallet_idx ON deposits (wallet_id);
CREATE        INDEX IF NOT EXISTS deposits_status_idx ON deposits (status) WHERE status != 'processed';

-- ── withdrawals ───────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS withdrawals (
    id                  TEXT        PRIMARY KEY,
    wallet_id           TEXT        NOT NULL REFERENCES wallets (id),
    account_id          TEXT        NOT NULL,
    to_address          TEXT        NOT NULL,
    chain_id            INTEGER     NOT NULL,
    amount_minor_units  BIGINT      NOT NULL CHECK (amount_minor_units > 0),
    fee_minor_units     BIGINT      NOT NULL DEFAULT 0,
    tx_hash             TEXT        NOT NULL DEFAULT '',
    status              TEXT        NOT NULL DEFAULT 'pending',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS withdrawals_wallet_idx  ON withdrawals (wallet_id);
CREATE INDEX IF NOT EXISTS withdrawals_status_idx  ON withdrawals (status) WHERE status = 'pending';

COMMIT;

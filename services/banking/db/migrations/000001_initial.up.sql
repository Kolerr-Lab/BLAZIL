-- Blazil Banking — initial schema
-- Run with: psql $BANKING_DATABASE_URL -f 001_initial.sql
-- Idempotent: all statements use IF NOT EXISTS / ON CONFLICT DO NOTHING.

BEGIN;

-- ── accounts ──────────────────────────────────────────────────────────────────
-- type:   0 = checking, 1 = savings, 2 = loan  (matches domain.AccountType iota)
-- status: 0 = active, 1 = closed, 2 = frozen   (matches domain.AccountStatus iota)
CREATE TABLE IF NOT EXISTS accounts (
    id                    TEXT        PRIMARY KEY,
    owner_id              TEXT        NOT NULL,
    type                  SMALLINT    NOT NULL DEFAULT 0,
    currency_code         TEXT        NOT NULL,
    balance_minor_units   BIGINT      NOT NULL DEFAULT 0,
    status                SMALLINT    NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS accounts_owner_idx ON accounts (owner_id);

-- ── balances ──────────────────────────────────────────────────────────────────
-- One row per account; upserted on every Debit/Credit.
CREATE TABLE IF NOT EXISTS balances (
    account_id     TEXT        PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE,
    minor_units    BIGINT      NOT NULL DEFAULT 0,
    currency_code  TEXT        NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── transactions ──────────────────────────────────────────────────────────────
-- type: 0 = debit, 1 = credit, 2 = interest, 3 = fee  (domain.TransactionType iota)
CREATE TABLE IF NOT EXISTS transactions (
    id                        TEXT        PRIMARY KEY,
    account_id                TEXT        NOT NULL REFERENCES accounts (id),
    type                      SMALLINT    NOT NULL,
    amount_minor_units        BIGINT      NOT NULL CHECK (amount_minor_units >= 0),
    currency_code             TEXT        NOT NULL,
    balance_after_minor_units BIGINT      NOT NULL,
    description               TEXT        NOT NULL DEFAULT '',
    reference                 TEXT        NOT NULL DEFAULT '',
    timestamp                 TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS transactions_account_idx ON transactions (account_id, timestamp DESC);

COMMIT;

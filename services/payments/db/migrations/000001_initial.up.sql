-- payments: persists every payment's full lifecycle state.
-- monetary amounts are stored as integer minor units (no floats).
-- metadata is an arbitrary caller-supplied key-value map (JSONB).

CREATE TABLE IF NOT EXISTS payments (
    id                TEXT        NOT NULL,
    idempotency_key   TEXT        NOT NULL,
    debit_account_id  TEXT        NOT NULL,
    credit_account_id TEXT        NOT NULL,
    amount_minor_units BIGINT     NOT NULL,
    currency_code     VARCHAR(8)  NOT NULL,
    currency_numeric  SMALLINT    NOT NULL,
    currency_decimals SMALLINT    NOT NULL,
    ledger_id         INTEGER     NOT NULL,
    rails             SMALLINT    NOT NULL,
    status            SMALLINT    NOT NULL,
    failure_reason    TEXT        NOT NULL DEFAULT '',
    metadata          JSONB       NOT NULL DEFAULT '{}',
    created_at        TIMESTAMPTZ NOT NULL,
    updated_at        TIMESTAMPTZ NOT NULL,

    CONSTRAINT payments_pkey PRIMARY KEY (id)
);

-- fast idempotency lookups (used on every incoming request)
CREATE UNIQUE INDEX IF NOT EXISTS payments_idempotency_key_idx
    ON payments (idempotency_key);

-- dashboard / reporting queries by status + recency
CREATE INDEX IF NOT EXISTS payments_status_created_idx
    ON payments (status, created_at DESC);

-- account-level history queries
CREATE INDEX IF NOT EXISTS payments_debit_account_idx
    ON payments (debit_account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS payments_credit_account_idx
    ON payments (credit_account_id, created_at DESC);

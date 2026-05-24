-- Blazil Trading — initial schema
-- Run with: psql $TRADING_DATABASE_URL -f 001_initial.sql
-- Idempotent: all statements use IF NOT EXISTS / ON CONFLICT DO NOTHING.

BEGIN;

-- ── orders ────────────────────────────────────────────────────────────────────
-- side:   1 = buy, 2 = sell  (matches domain.Side iota)
-- status: 1 = open, 2 = partial, 3 = filled, 4 = cancelled
CREATE TABLE IF NOT EXISTS orders (
    id                      TEXT        PRIMARY KEY,
    instrument_id           TEXT        NOT NULL,
    owner_id                TEXT        NOT NULL,
    side                    SMALLINT    NOT NULL CHECK (side IN (1, 2)),
    limit_price_minor_units BIGINT      NOT NULL CHECK (limit_price_minor_units > 0),
    quantity_units          BIGINT      NOT NULL CHECK (quantity_units > 0),
    filled_units            BIGINT      NOT NULL DEFAULT 0 CHECK (filled_units >= 0),
    status                  SMALLINT    NOT NULL DEFAULT 1,
    placed_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS orders_owner_idx        ON orders (owner_id);
CREATE INDEX IF NOT EXISTS orders_instrument_idx   ON orders (instrument_id);
CREATE INDEX IF NOT EXISTS orders_status_idx       ON orders (status) WHERE status IN (1, 2);

-- ── trades ────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS trades (
    id               TEXT        PRIMARY KEY,
    instrument_id    TEXT        NOT NULL,
    maker_order_id   TEXT        NOT NULL REFERENCES orders (id),
    taker_order_id   TEXT        NOT NULL REFERENCES orders (id),
    price_minor_units BIGINT     NOT NULL,
    quantity_units   BIGINT      NOT NULL,
    executed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS trades_maker_idx  ON trades (maker_order_id);
CREATE INDEX IF NOT EXISTS trades_taker_idx  ON trades (taker_order_id);
CREATE INDEX IF NOT EXISTS trades_instr_idx  ON trades (instrument_id, executed_at DESC);

COMMIT;

package balances

import (
	"context"
	"fmt"
	"time"

	"github.com/blazil/banking/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgBalanceStore is a PostgreSQL-backed BalanceStore.
type PgBalanceStore struct {
	db *pgxpool.Pool
}

// NewPgBalanceStore constructs a PgBalanceStore backed by the given pool.
func NewPgBalanceStore(db *pgxpool.Pool) *PgBalanceStore {
	return &PgBalanceStore{db: db}
}

// Get implements BalanceStore.
func (s *PgBalanceStore) Get(ctx context.Context, id domain.AccountID) (*domain.Balance, error) {
	const q = `SELECT account_id, minor_units, currency_code, updated_at FROM balances WHERE account_id = $1`
	var b domain.Balance
	err := s.db.QueryRow(ctx, q, string(id)).Scan(
		&b.AccountID, &b.MinorUnits, &b.CurrencyCode, &b.UpdatedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrAccountNotFound
		}
		return nil, fmt.Errorf("get balance: %w", err)
	}
	return &b, nil
}

// Set implements BalanceStore (upsert).
func (s *PgBalanceStore) Set(ctx context.Context, bal *domain.Balance) error {
	const q = `
		INSERT INTO balances (account_id, minor_units, currency_code, updated_at)
		VALUES ($1, $2, $3, $4)
		ON CONFLICT (account_id) DO UPDATE
		  SET minor_units   = EXCLUDED.minor_units,
		      currency_code = EXCLUDED.currency_code,
		      updated_at    = EXCLUDED.updated_at`
	if _, err := s.db.Exec(ctx, q,
		string(bal.AccountID), bal.MinorUnits, bal.CurrencyCode, time.Now().UTC(),
	); err != nil {
		return fmt.Errorf("set balance: %w", err)
	}
	return nil
}

// compile-time interface check.
var _ BalanceStore = (*PgBalanceStore)(nil)

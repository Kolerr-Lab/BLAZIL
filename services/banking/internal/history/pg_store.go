package history

import (
	"context"
	"fmt"

	"github.com/blazil/banking/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgTransactionStore is a PostgreSQL-backed TransactionStore.
type PgTransactionStore struct {
	db *pgxpool.Pool
}

// NewPgTransactionStore constructs a PgTransactionStore backed by the given pool.
func NewPgTransactionStore(db *pgxpool.Pool) *PgTransactionStore {
	return &PgTransactionStore{db: db}
}

// Append implements TransactionStore.
func (s *PgTransactionStore) Append(ctx context.Context, tx *domain.Transaction) error {
	const q = `
		INSERT INTO transactions
			(id, account_id, type, amount_minor_units, currency_code,
			 balance_after_minor_units, description, reference, timestamp)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)`
	if _, err := s.db.Exec(ctx, q,
		string(tx.ID), string(tx.AccountID), int(tx.Type),
		tx.AmountMinorUnits, tx.CurrencyCode,
		tx.BalanceAfterMinorUnits, tx.Description, tx.Reference, tx.Timestamp,
	); err != nil {
		return fmt.Errorf("append transaction: %w", err)
	}
	return nil
}

// GetByID implements TransactionStore.
func (s *PgTransactionStore) GetByID(ctx context.Context, id domain.TransactionID) (*domain.Transaction, error) {
	const q = `
		SELECT id, account_id, type, amount_minor_units, currency_code,
		       balance_after_minor_units, description, reference, timestamp
		FROM transactions WHERE id = $1`
	var tx domain.Transaction
	err := s.db.QueryRow(ctx, q, string(id)).Scan(
		&tx.ID, &tx.AccountID, &tx.Type,
		&tx.AmountMinorUnits, &tx.CurrencyCode,
		&tx.BalanceAfterMinorUnits, &tx.Description, &tx.Reference, &tx.Timestamp,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrTransactionNotFound
		}
		return nil, fmt.Errorf("get transaction: %w", err)
	}
	return &tx, nil
}

// ListByAccount implements TransactionStore.
func (s *PgTransactionStore) ListByAccount(ctx context.Context, accountID domain.AccountID, opts ListOptions) ([]*domain.Transaction, error) {
	const q = `
		SELECT id, account_id, type, amount_minor_units, currency_code,
		       balance_after_minor_units, description, reference, timestamp
		FROM transactions
		WHERE account_id = $1
		ORDER BY timestamp DESC
		LIMIT NULLIF($2, 0) OFFSET $3`

	rows, err := s.db.Query(ctx, q, string(accountID), opts.Limit, opts.Offset)
	if err != nil {
		return nil, fmt.Errorf("list transactions: %w", err)
	}
	defer rows.Close()

	var out []*domain.Transaction
	for rows.Next() {
		var tx domain.Transaction
		if err := rows.Scan(
			&tx.ID, &tx.AccountID, &tx.Type,
			&tx.AmountMinorUnits, &tx.CurrencyCode,
			&tx.BalanceAfterMinorUnits, &tx.Description, &tx.Reference, &tx.Timestamp,
		); err != nil {
			return nil, err
		}
		out = append(out, &tx)
	}
	return out, rows.Err()
}

// compile-time interface check.
var _ TransactionStore = (*PgTransactionStore)(nil)

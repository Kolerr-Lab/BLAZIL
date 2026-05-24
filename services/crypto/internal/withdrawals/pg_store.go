package withdrawals

import (
	"context"
	"fmt"

	"github.com/blazil/crypto/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgWithdrawalStore is a PostgreSQL-backed WithdrawalStore.
type PgWithdrawalStore struct {
	db *pgxpool.Pool
}

// NewPgWithdrawalStore constructs a PgWithdrawalStore backed by the given pool.
func NewPgWithdrawalStore(db *pgxpool.Pool) *PgWithdrawalStore {
	return &PgWithdrawalStore{db: db}
}

// Save implements WithdrawalStore (upsert).
func (s *PgWithdrawalStore) Save(ctx context.Context, w *domain.Withdrawal) error {
	const q = `
		INSERT INTO withdrawals
			(id, wallet_id, account_id, to_address, chain_id,
			 amount_minor_units, fee_minor_units, tx_hash, status, created_at)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
		ON CONFLICT (id) DO UPDATE
		  SET tx_hash = EXCLUDED.tx_hash,
		      status  = EXCLUDED.status`
	if _, err := s.db.Exec(ctx, q,
		w.ID, w.WalletID, w.AccountID, w.ToAddress, int32(w.ChainID),
		w.AmountMinorUnits, w.FeeMinorUnits, w.TxHash,
		string(w.Status), w.CreatedAt,
	); err != nil {
		return fmt.Errorf("save withdrawal: %w", err)
	}
	return nil
}

// FindByID implements WithdrawalStore.
func (s *PgWithdrawalStore) FindByID(ctx context.Context, id string) (*domain.Withdrawal, error) {
	const q = `
		SELECT id, wallet_id, account_id, to_address, chain_id,
		       amount_minor_units, fee_minor_units, tx_hash, status, created_at
		FROM withdrawals WHERE id = $1`
	var w domain.Withdrawal
	err := s.db.QueryRow(ctx, q, id).Scan(
		&w.ID, &w.WalletID, &w.AccountID, &w.ToAddress, &w.ChainID,
		&w.AmountMinorUnits, &w.FeeMinorUnits, &w.TxHash,
		&w.Status, &w.CreatedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrWithdrawalNotFound
		}
		return nil, fmt.Errorf("find withdrawal: %w", err)
	}
	return &w, nil
}

// compile-time interface check.
var _ WithdrawalStore = (*PgWithdrawalStore)(nil)

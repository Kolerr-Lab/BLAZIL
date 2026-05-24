package deposits

import (
	"context"
	"fmt"

	"github.com/blazil/crypto/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgDepositStore is a PostgreSQL-backed DepositStore.
type PgDepositStore struct {
	db *pgxpool.Pool
}

// NewPgDepositStore constructs a PgDepositStore backed by the given pool.
func NewPgDepositStore(db *pgxpool.Pool) *PgDepositStore {
	return &PgDepositStore{db: db}
}

// Save implements DepositStore (upsert).
func (s *PgDepositStore) Save(ctx context.Context, d *domain.Deposit) error {
	const q = `
		INSERT INTO deposits
			(id, wallet_id, account_id, tx_hash, chain_id, amount_minor_units,
			 status, confirmations, created_at, processed_at)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
		ON CONFLICT (id) DO UPDATE
		  SET status         = EXCLUDED.status,
		      confirmations  = EXCLUDED.confirmations,
		      processed_at   = EXCLUDED.processed_at`
	if _, err := s.db.Exec(ctx, q,
		d.ID, d.WalletID, d.AccountID, d.TxHash, int32(d.ChainID),
		d.AmountMinorUnits, string(d.Status), d.Confirmations,
		d.CreatedAt, d.ProcessedAt,
	); err != nil {
		return fmt.Errorf("save deposit: %w", err)
	}
	return nil
}

// FindByID implements DepositStore.
func (s *PgDepositStore) FindByID(ctx context.Context, id string) (*domain.Deposit, error) {
	const q = `
		SELECT id, wallet_id, account_id, tx_hash, chain_id, amount_minor_units,
		       status, confirmations, created_at, processed_at
		FROM deposits WHERE id = $1`
	var d domain.Deposit
	err := s.db.QueryRow(ctx, q, id).Scan(
		&d.ID, &d.WalletID, &d.AccountID, &d.TxHash, &d.ChainID,
		&d.AmountMinorUnits, &d.Status, &d.Confirmations,
		&d.CreatedAt, &d.ProcessedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrDepositNotFound
		}
		return nil, fmt.Errorf("find deposit: %w", err)
	}
	return &d, nil
}

// FindByTxHash implements DepositStore.
func (s *PgDepositStore) FindByTxHash(ctx context.Context, txHash string) (*domain.Deposit, error) {
	const q = `
		SELECT id, wallet_id, account_id, tx_hash, chain_id, amount_minor_units,
		       status, confirmations, created_at, processed_at
		FROM deposits WHERE tx_hash = $1 LIMIT 1`
	var d domain.Deposit
	err := s.db.QueryRow(ctx, q, txHash).Scan(
		&d.ID, &d.WalletID, &d.AccountID, &d.TxHash, &d.ChainID,
		&d.AmountMinorUnits, &d.Status, &d.Confirmations,
		&d.CreatedAt, &d.ProcessedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrDepositNotFound
		}
		return nil, fmt.Errorf("find deposit by txhash: %w", err)
	}
	return &d, nil
}

// compile-time interface check.
var _ DepositStore = (*PgDepositStore)(nil)

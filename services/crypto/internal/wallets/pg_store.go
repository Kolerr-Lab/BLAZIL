package wallets

import (
	"context"
	"fmt"

	"github.com/blazil/crypto/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgWalletStore is a PostgreSQL-backed WalletStore.
type PgWalletStore struct {
	db *pgxpool.Pool
}

// NewPgWalletStore constructs a PgWalletStore backed by the given pool.
func NewPgWalletStore(db *pgxpool.Pool) *PgWalletStore {
	return &PgWalletStore{db: db}
}

// Save implements WalletStore (upsert).
func (s *PgWalletStore) Save(ctx context.Context, w *domain.Wallet) error {
	const q = `
		INSERT INTO wallets (id, owner_id, chain_id, address, type, status)
		VALUES ($1,$2,$3,$4,$5,$6)
		ON CONFLICT (id) DO UPDATE
		  SET status = EXCLUDED.status`
	if _, err := s.db.Exec(ctx, q,
		w.ID, w.OwnerID, int32(w.ChainID), w.Address,
		string(w.Type), string(w.Status),
	); err != nil {
		return fmt.Errorf("save wallet: %w", err)
	}
	return nil
}

// FindByID implements WalletStore.
func (s *PgWalletStore) FindByID(ctx context.Context, id string) (*domain.Wallet, error) {
	const q = `SELECT id, owner_id, chain_id, address, type, status FROM wallets WHERE id = $1`
	var w domain.Wallet
	err := s.db.QueryRow(ctx, q, id).Scan(
		&w.ID, &w.OwnerID, &w.ChainID, &w.Address, &w.Type, &w.Status,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrWalletNotFound
		}
		return nil, fmt.Errorf("find wallet: %w", err)
	}
	return &w, nil
}

// FindByOwner implements WalletStore.
func (s *PgWalletStore) FindByOwner(ctx context.Context, ownerID string) ([]*domain.Wallet, error) {
	const q = `SELECT id, owner_id, chain_id, address, type, status FROM wallets WHERE owner_id = $1`
	rows, err := s.db.Query(ctx, q, ownerID)
	if err != nil {
		return nil, fmt.Errorf("find wallets by owner: %w", err)
	}
	defer rows.Close()
	var out []*domain.Wallet
	for rows.Next() {
		var w domain.Wallet
		if err := rows.Scan(&w.ID, &w.OwnerID, &w.ChainID, &w.Address, &w.Type, &w.Status); err != nil {
			return nil, err
		}
		out = append(out, &w)
	}
	return out, rows.Err()
}

// compile-time interface check.
var _ WalletStore = (*PgWalletStore)(nil)

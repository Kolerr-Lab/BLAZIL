package accounts

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/blazil/banking/internal/domain"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgAccountService is a PostgreSQL-backed AccountService.
type PgAccountService struct {
	db *pgxpool.Pool

	bcMu           sync.RWMutex
	balanceChecker BalanceChecker
}

// NewPgAccountService constructs a PgAccountService backed by the given pool.
func NewPgAccountService(db *pgxpool.Pool) *PgAccountService {
	return &PgAccountService{db: db}
}

// SetBalanceService implements AccountService.
func (s *PgAccountService) SetBalanceService(bc BalanceChecker) {
	s.bcMu.Lock()
	s.balanceChecker = bc
	s.bcMu.Unlock()
}

// CreateAccount implements AccountService.
func (s *PgAccountService) CreateAccount(ctx context.Context, req CreateAccountRequest) (*domain.Account, error) {
	if req.InitialBalanceMinorUnits < 0 {
		return nil, domain.ErrNegativeAmount
	}
	now := time.Now().UTC()
	acc := &domain.Account{
		ID:                req.ID,
		OwnerID:           req.OwnerID,
		Type:              req.Type,
		CurrencyCode:      req.CurrencyCode,
		BalanceMinorUnits: req.InitialBalanceMinorUnits,
		Status:            domain.AccountStatusActive,
		CreatedAt:         now,
		UpdatedAt:         now,
	}
	const q = `
		INSERT INTO accounts
			(id, owner_id, type, currency_code, balance_minor_units, status, created_at, updated_at)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8)`
	if _, err := s.db.Exec(ctx, q,
		string(acc.ID), acc.OwnerID, int(acc.Type), acc.CurrencyCode,
		acc.BalanceMinorUnits, int(acc.Status), acc.CreatedAt, acc.UpdatedAt,
	); err != nil {
		if strings.Contains(err.Error(), "23505") {
			return nil, domain.ErrAccountAlreadyExists
		}
		return nil, fmt.Errorf("create account: %w", err)
	}
	return acc, nil
}

// GetAccount implements AccountService.
func (s *PgAccountService) GetAccount(ctx context.Context, id domain.AccountID) (*domain.Account, error) {
	const q = `
		SELECT id, owner_id, type, currency_code, balance_minor_units, status, created_at, updated_at
		FROM accounts WHERE id = $1`
	var a domain.Account
	err := s.db.QueryRow(ctx, q, string(id)).Scan(
		&a.ID, &a.OwnerID, &a.Type, &a.CurrencyCode,
		&a.BalanceMinorUnits, &a.Status, &a.CreatedAt, &a.UpdatedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, fmt.Errorf("account %s: %w", id, domain.ErrAccountNotFound)
		}
		return nil, fmt.Errorf("get account: %w", err)
	}
	return &a, nil
}

// ListAccountsByOwner implements AccountService.
func (s *PgAccountService) ListAccountsByOwner(ctx context.Context, ownerID string) ([]*domain.Account, error) {
	const q = `
		SELECT id, owner_id, type, currency_code, balance_minor_units, status, created_at, updated_at
		FROM accounts WHERE owner_id = $1 ORDER BY created_at ASC`
	rows, err := s.db.Query(ctx, q, ownerID)
	if err != nil {
		return nil, fmt.Errorf("list accounts: %w", err)
	}
	defer rows.Close()
	var out []*domain.Account
	for rows.Next() {
		var a domain.Account
		if err := rows.Scan(
			&a.ID, &a.OwnerID, &a.Type, &a.CurrencyCode,
			&a.BalanceMinorUnits, &a.Status, &a.CreatedAt, &a.UpdatedAt,
		); err != nil {
			return nil, err
		}
		out = append(out, &a)
	}
	return out, rows.Err()
}

// FreezeAccount implements AccountService.
func (s *PgAccountService) FreezeAccount(ctx context.Context, id domain.AccountID) error {
	tag, err := s.db.Exec(ctx,
		`UPDATE accounts SET status = $1, updated_at = now() WHERE id = $2 AND status = $3`,
		int(domain.AccountStatusFrozen), string(id), int(domain.AccountStatusActive))
	if err != nil {
		return fmt.Errorf("freeze account: %w", err)
	}
	if tag.RowsAffected() == 0 {
		// Distinguish between not-found and already-frozen/closed by fetching.
		acc, err2 := s.GetAccount(ctx, id)
		if err2 != nil {
			return err2
		}
		switch acc.Status {
		case domain.AccountStatusFrozen:
			return domain.ErrAccountFrozen
		case domain.AccountStatusClosed:
			return domain.ErrAccountClosed
		}
	}
	return nil
}

// CloseAccount implements AccountService.
func (s *PgAccountService) CloseAccount(ctx context.Context, id domain.AccountID) error {
	acc, err := s.GetAccount(ctx, id)
	if err != nil {
		return err
	}
	if acc.Status == domain.AccountStatusClosed {
		return domain.ErrAccountClosed
	}

	s.bcMu.RLock()
	bc := s.balanceChecker
	s.bcMu.RUnlock()
	if bc != nil {
		bal, err := bc.GetBalance(ctx, id)
		if err == nil && bal.MinorUnits != 0 {
			return domain.ErrAccountHasBalance
		}
	}

	if _, err := s.db.Exec(ctx,
		`UPDATE accounts SET status = $1, updated_at = now() WHERE id = $2`,
		int(domain.AccountStatusClosed), string(id),
	); err != nil {
		return fmt.Errorf("close account: %w", err)
	}
	return nil
}

// compile-time interface check.
var _ AccountService = (*PgAccountService)(nil)

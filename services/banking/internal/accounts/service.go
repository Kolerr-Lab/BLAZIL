// Package accounts implements account management for the Blazil banking service.
package accounts

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/banking/internal/domain"
)

// BalanceChecker is a narrow interface used by AccountService to query the
// canonical balance before closing an account.  It is defined here (rather than
// in the balances package) to avoid a circular import.
type BalanceChecker interface {
	GetBalance(ctx context.Context, id domain.AccountID) (*domain.Balance, error)
}

// AccountService manages bank account lifecycle operations.
// All implementations must be safe for concurrent use.
type AccountService interface {
	// CreateAccount opens a new account and returns the persisted record.
	CreateAccount(ctx context.Context, req CreateAccountRequest) (*domain.Account, error)

	// GetAccount returns the account for the given ID.
	GetAccount(ctx context.Context, id domain.AccountID) (*domain.Account, error)

	// ListAccountsByOwner returns all accounts belonging to ownerID.
	ListAccountsByOwner(ctx context.Context, ownerID string) ([]*domain.Account, error)

	// FreezeAccount transitions an active account to frozen status.
	// Returns ErrAccountFrozen if already frozen, ErrAccountClosed if closed.
	FreezeAccount(ctx context.Context, id domain.AccountID) error

	// CloseAccount transitions the account to closed status.
	// If a BalanceChecker has been injected via SetBalanceService, the balance
	// must be zero; otherwise ErrAccountHasBalance is returned.
	CloseAccount(ctx context.Context, id domain.AccountID) error

	// SetBalanceService injects the BalanceChecker used by CloseAccount.
	// Call this once during wiring (after both services are constructed).
	SetBalanceService(bc BalanceChecker)
}

// CreateAccountRequest is the input for AccountService.CreateAccount.
type CreateAccountRequest struct {
	// ID is the caller-supplied account ID (must be unique).
	ID domain.AccountID

	// OwnerID is the external customer identifier.
	OwnerID string

	// Type is the account classification.
	Type domain.AccountType

	// CurrencyCode is the ISO 4217 code, e.g. "USD".
	CurrencyCode string

	// InitialBalanceMinorUnits is the opening balance (must be >= 0).
	InitialBalanceMinorUnits int64
}

// InMemoryAccountService is a thread-safe in-memory implementation of AccountService.
type InMemoryAccountService struct {
	mu             sync.RWMutex
	accounts       map[domain.AccountID]*domain.Account
	bcMu           sync.RWMutex
	balanceChecker BalanceChecker
}

// NewInMemoryAccountService constructs an empty InMemoryAccountService.
func NewInMemoryAccountService() *InMemoryAccountService {
	return &InMemoryAccountService{
		accounts: make(map[domain.AccountID]*domain.Account),
	}
}

// SetBalanceService implements AccountService.
func (s *InMemoryAccountService) SetBalanceService(bc BalanceChecker) {
	s.bcMu.Lock()
	s.balanceChecker = bc
	s.bcMu.Unlock()
}

// CreateAccount implements AccountService.
func (s *InMemoryAccountService) CreateAccount(_ context.Context, req CreateAccountRequest) (*domain.Account, error) {
	if req.InitialBalanceMinorUnits < 0 {
		return nil, domain.ErrNegativeAmount
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	if _, exists := s.accounts[req.ID]; exists {
		return nil, domain.ErrAccountAlreadyExists
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
	s.accounts[req.ID] = acc
	return acc, nil
}

// GetAccount implements AccountService.
func (s *InMemoryAccountService) GetAccount(_ context.Context, id domain.AccountID) (*domain.Account, error) {
	s.mu.RLock()
	acc, ok := s.accounts[id]
	s.mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("account %s: %w", id, domain.ErrAccountNotFound)
	}
	return acc, nil
}

// ListAccountsByOwner implements AccountService.
func (s *InMemoryAccountService) ListAccountsByOwner(_ context.Context, ownerID string) ([]*domain.Account, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var result []*domain.Account
	for _, acc := range s.accounts {
		if acc.OwnerID == ownerID {
			result = append(result, acc)
		}
	}
	return result, nil
}

// FreezeAccount implements AccountService.
func (s *InMemoryAccountService) FreezeAccount(_ context.Context, id domain.AccountID) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	acc, ok := s.accounts[id]
	if !ok {
		return fmt.Errorf("account %s: %w", id, domain.ErrAccountNotFound)
	}
	switch acc.Status {
	case domain.AccountStatusFrozen:
		return domain.ErrAccountFrozen
	case domain.AccountStatusClosed:
		return domain.ErrAccountClosed
	}
	acc.Status = domain.AccountStatusFrozen
	acc.UpdatedAt = time.Now().UTC()
	return nil
}

// CloseAccount implements AccountService.
// If a BalanceChecker has been injected, the balance must be zero.
func (s *InMemoryAccountService) CloseAccount(ctx context.Context, id domain.AccountID) error {
	// Read the balance checker under its own lock before acquiring the account lock.
	s.bcMu.RLock()
	bc := s.balanceChecker
	s.bcMu.RUnlock()

	if bc != nil {
		bal, err := bc.GetBalance(ctx, id)
		if err != nil {
			return err
		}
		if bal.MinorUnits != 0 {
			return domain.ErrAccountHasBalance
		}
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	acc, ok := s.accounts[id]
	if !ok {
		return fmt.Errorf("account %s: %w", id, domain.ErrAccountNotFound)
	}
	if acc.Status == domain.AccountStatusClosed {
		return domain.ErrAccountClosed
	}
	acc.Status = domain.AccountStatusClosed
	acc.UpdatedAt = time.Now().UTC()
	return nil
}

// Package balances implements balance management for the Blazil banking service.
package balances

import (
	"context"
	"fmt"
	"sync"

	"github.com/blazil/banking/internal/domain"
)

// BalanceStore persists and retrieves Balance records.
// All implementations must be safe for concurrent use.
type BalanceStore interface {
	// Get returns the Balance for the given account, or ErrAccountNotFound.
	Get(ctx context.Context, id domain.AccountID) (*domain.Balance, error)

	// Set creates or replaces the Balance for the given account.
	Set(ctx context.Context, bal *domain.Balance) error
}

// InMemoryBalanceStore is a thread-safe in-memory BalanceStore.
type InMemoryBalanceStore struct {
	mu       sync.RWMutex
	balances map[domain.AccountID]*domain.Balance
}

// NewInMemoryBalanceStore constructs an empty InMemoryBalanceStore.
func NewInMemoryBalanceStore() *InMemoryBalanceStore {
	return &InMemoryBalanceStore{
		balances: make(map[domain.AccountID]*domain.Balance),
	}
}

// Get implements BalanceStore.
func (s *InMemoryBalanceStore) Get(_ context.Context, id domain.AccountID) (*domain.Balance, error) {
	s.mu.RLock()
	bal, ok := s.balances[id]
	s.mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("balance for account %s: %w", id, domain.ErrAccountNotFound)
	}
	return bal, nil
}

// Set implements BalanceStore.
func (s *InMemoryBalanceStore) Set(_ context.Context, bal *domain.Balance) error {
	s.mu.Lock()
	s.balances[bal.AccountID] = bal
	s.mu.Unlock()
	return nil
}

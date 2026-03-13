// Package history implements transaction history storage for the Blazil banking service.
package history

import (
	"context"
	"fmt"
	"sort"
	"sync"

	"github.com/blazil/banking/internal/domain"
)

// InMemoryTransactionStore is a thread-safe in-memory implementation of TransactionStore.
type InMemoryTransactionStore struct {
	mu        sync.RWMutex
	byID      map[domain.TransactionID]*domain.Transaction
	byAccount map[domain.AccountID][]*domain.Transaction
}

// NewInMemoryTransactionStore constructs an empty InMemoryTransactionStore.
func NewInMemoryTransactionStore() *InMemoryTransactionStore {
	return &InMemoryTransactionStore{
		byID:      make(map[domain.TransactionID]*domain.Transaction),
		byAccount: make(map[domain.AccountID][]*domain.Transaction),
	}
}

// Append implements TransactionStore.
func (s *InMemoryTransactionStore) Append(_ context.Context, tx *domain.Transaction) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, exists := s.byID[tx.ID]; exists {
		return fmt.Errorf("transaction %s already exists", tx.ID)
	}
	s.byID[tx.ID] = tx
	s.byAccount[tx.AccountID] = append(s.byAccount[tx.AccountID], tx)
	return nil
}

// GetByID implements TransactionStore.
func (s *InMemoryTransactionStore) GetByID(_ context.Context, id domain.TransactionID) (*domain.Transaction, error) {
	s.mu.RLock()
	tx, ok := s.byID[id]
	s.mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("transaction %s: %w", id, domain.ErrTransactionNotFound)
	}
	return tx, nil
}

// ListByAccount implements TransactionStore. Results are sorted newest-first.
func (s *InMemoryTransactionStore) ListByAccount(_ context.Context, accountID domain.AccountID, opts ListOptions) ([]*domain.Transaction, error) {
	s.mu.RLock()
	txs := make([]*domain.Transaction, len(s.byAccount[accountID]))
	copy(txs, s.byAccount[accountID])
	s.mu.RUnlock()

	// Sort descending by timestamp.
	sort.Slice(txs, func(i, j int) bool {
		return txs[i].Timestamp.After(txs[j].Timestamp)
	})

	// Apply offset + limit.
	if opts.Offset >= len(txs) {
		return nil, nil
	}
	txs = txs[opts.Offset:]
	if opts.Limit > 0 && opts.Limit < len(txs) {
		txs = txs[:opts.Limit]
	}
	return txs, nil
}

// Package lifecycle orchestrates the payment processing lifecycle.
package lifecycle

import (
	"fmt"
	"sync"

	"github.com/blazil/services/payments/internal/domain"
)

// PaymentStore persists and retrieves Payment records.
// All implementations must be safe for concurrent use.
type PaymentStore interface {
	// GetByID returns the payment with the given ID, or ErrPaymentNotFound.
	GetByID(id domain.PaymentID) (*domain.Payment, error)

	// GetByIdempotencyKey returns the payment for the given key, or nil if not found.
	// A missing key is not an error (callers use nil to decide whether to process).
	GetByIdempotencyKey(key string) (*domain.Payment, error)

	// Save inserts or updates both the by-ID and by-idempotency-key indexes atomically.
	Save(payment *domain.Payment) error
}

// InMemoryPaymentStore is a thread-safe in-memory implementation of PaymentStore.
type InMemoryPaymentStore struct {
	mu            sync.RWMutex
	byID          map[domain.PaymentID]*domain.Payment
	byIdempotency map[string]*domain.Payment
}

// NewInMemoryPaymentStore constructs an empty InMemoryPaymentStore.
func NewInMemoryPaymentStore() *InMemoryPaymentStore {
	return &InMemoryPaymentStore{
		byID:          make(map[domain.PaymentID]*domain.Payment),
		byIdempotency: make(map[string]*domain.Payment),
	}
}

// GetByID implements PaymentStore.
func (s *InMemoryPaymentStore) GetByID(id domain.PaymentID) (*domain.Payment, error) {
	s.mu.RLock()
	p, ok := s.byID[id]
	s.mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("payment %s: %w", id, domain.ErrPaymentNotFound)
	}
	return p, nil
}

// GetByIdempotencyKey implements PaymentStore.
func (s *InMemoryPaymentStore) GetByIdempotencyKey(key string) (*domain.Payment, error) {
	s.mu.RLock()
	p := s.byIdempotency[key]
	s.mu.RUnlock()
	return p, nil
}

// Save implements PaymentStore.
// Both indexes are updated atomically under a single write lock.
func (s *InMemoryPaymentStore) Save(payment *domain.Payment) error {
	s.mu.Lock()
	s.byID[payment.ID] = payment
	if payment.IdempotencyKey != "" {
		s.byIdempotency[payment.IdempotencyKey] = payment
	}
	s.mu.Unlock()
	return nil
}

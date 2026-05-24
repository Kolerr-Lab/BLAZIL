// Package db provides database utilities for the Blazil payments service.
package db

import (
	"go.uber.org/zap"

	"github.com/blazil/services/payments/internal/domain"
)

// PostgresIdempotencyStore implements lifecycle.IdempotencyStore on top of the
// payments table via PostgresPaymentStore.
//
// Get queries the payments table directly using the idempotency key index.
// Set is intentionally a no-op: by the time Set is called in the payment
// processor lifecycle, the payment has already been persisted to Postgres by
// PaymentStore.Save — re-inserting it would be redundant.
//
// On any DB error in Get, the method returns nil (treats as a cache miss) so
// the processor re-runs and idempotency is guaranteed at the store level.
type PostgresIdempotencyStore struct {
	store  *PostgresPaymentStore
	logger *zap.Logger
}

// NewPostgresIdempotencyStore wraps an existing PostgresPaymentStore.
func NewPostgresIdempotencyStore(store *PostgresPaymentStore, logger *zap.Logger) *PostgresIdempotencyStore {
	return &PostgresIdempotencyStore{store: store, logger: logger}
}

// Get implements lifecycle.IdempotencyStore.
// Returns nil on DB error or key not found.
func (s *PostgresIdempotencyStore) Get(key string) *domain.Payment {
	p, err := s.store.GetByIdempotencyKey(key)
	if err != nil {
		s.logger.Warn("idempotency lookup failed; treating as cache miss",
			zap.String("key", key),
			zap.Error(err),
		)
		return nil
	}
	return p
}

// Set implements lifecycle.IdempotencyStore.
// No-op: the payment is already durable in Postgres via PaymentStore.Save.
func (s *PostgresIdempotencyStore) Set(_ string, _ *domain.Payment) {}

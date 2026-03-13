// Package lifecycle orchestrates the payment processing lifecycle.
package lifecycle

import (
	"sync"
	"time"

	"github.com/blazil/services/payments/internal/domain"
)

// IdempotencyStore provides deduplication for payment requests.
// Implementations must be safe for concurrent use.
type IdempotencyStore interface {
	// Get returns the cached payment for the given idempotency key, or nil.
	Get(key string) *domain.Payment

	// Set stores the payment under the given idempotency key.
	Set(key string, payment *domain.Payment)
}

// idempotencyEntry wraps a Payment with its store timestamp for TTL eviction.
type idempotencyEntry struct {
	payment  *domain.Payment
	storedAt time.Time
}

// InMemoryIdempotencyStore is a TTL-based in-memory implementation of IdempotencyStore.
// Entries expire after the configured TTL (default 24 hours).
// Safe for concurrent use.
type InMemoryIdempotencyStore struct {
	mu      sync.RWMutex
	entries map[string]*idempotencyEntry
	ttl     time.Duration
}

// NewInMemoryIdempotencyStore constructs a store with the given TTL.
// Use 24 * time.Hour for production.
func NewInMemoryIdempotencyStore(ttl time.Duration) *InMemoryIdempotencyStore {
	return &InMemoryIdempotencyStore{
		entries: make(map[string]*idempotencyEntry),
		ttl:     ttl,
	}
}

// Get implements IdempotencyStore.
// Returns nil if the key is not found or its entry has expired.
func (s *InMemoryIdempotencyStore) Get(key string) *domain.Payment {
	s.mu.RLock()
	entry, ok := s.entries[key]
	s.mu.RUnlock()

	if !ok {
		return nil
	}
	if time.Since(entry.storedAt) > s.ttl {
		s.mu.Lock()
		delete(s.entries, key)
		s.mu.Unlock()
		return nil
	}
	return entry.payment
}

// Set implements IdempotencyStore.
func (s *InMemoryIdempotencyStore) Set(key string, payment *domain.Payment) {
	s.mu.Lock()
	s.entries[key] = &idempotencyEntry{
		payment:  payment,
		storedAt: time.Now(),
	}
	s.mu.Unlock()
}

// StartCleanup begins a background goroutine that evicts expired entries on the
// given interval. The goroutine stops when the provided done channel is closed.
func (s *InMemoryIdempotencyStore) StartCleanup(interval time.Duration, done <-chan struct{}) {
	go func() {
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
				s.evictExpired()
			case <-done:
				return
			}
		}
	}()
}

// evictExpired removes entries whose TTL has elapsed.
func (s *InMemoryIdempotencyStore) evictExpired() {
	now := time.Now()
	s.mu.Lock()
	for key, entry := range s.entries {
		if now.Sub(entry.storedAt) > s.ttl {
			delete(s.entries, key)
		}
	}
	s.mu.Unlock()
}

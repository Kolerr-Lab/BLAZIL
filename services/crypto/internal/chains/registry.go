// Package chains defines the abstraction layer for blockchain interactions.
package chains

import (
	"sync"

	"github.com/blazil/crypto/internal/domain"
)

// ChainRegistry maps chain IDs to their adapters.
// Safe for concurrent use.
type ChainRegistry struct {
	mu       sync.RWMutex
	adapters map[domain.ChainID]ChainAdapter
}

// NewChainRegistry returns an empty ChainRegistry.
func NewChainRegistry() *ChainRegistry {
	return &ChainRegistry{
		adapters: make(map[domain.ChainID]ChainAdapter),
	}
}

// Register adds an adapter for a chain. Overwrites any existing adapter.
func (r *ChainRegistry) Register(a ChainAdapter) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.adapters[a.ChainID()] = a
}

// Get returns the adapter for the given chain ID.
// Returns (nil, ErrChainNotFound) if the chain is not registered.
func (r *ChainRegistry) Get(id domain.ChainID) (ChainAdapter, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	a, ok := r.adapters[id]
	if !ok {
		return nil, domain.ErrChainNotFound
	}
	return a, nil
}

// GetAll returns a slice of all registered adapters.
func (r *ChainRegistry) GetAll() []ChainAdapter {
	r.mu.RLock()
	defer r.mu.RUnlock()
	out := make([]ChainAdapter, 0, len(r.adapters))
	for _, a := range r.adapters {
		out = append(out, a)
	}
	return out
}

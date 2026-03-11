// Package wallets manages crypto wallet lifecycle.
package wallets

import (
	"context"
	"fmt"
	"sync"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
)

// WalletStore is the persistence interface for wallets.
type WalletStore interface {
	Save(ctx context.Context, w *domain.Wallet) error
	FindByID(ctx context.Context, id string) (*domain.Wallet, error)
	FindByOwner(ctx context.Context, ownerID string) ([]*domain.Wallet, error)
}

// WalletService is the application-level interface for wallet operations.
type WalletService interface {
	CreateWallet(ctx context.Context, req CreateWalletRequest) (*domain.Wallet, error)
	GetWallet(ctx context.Context, id string) (*domain.Wallet, error)
	ListWalletsByOwner(ctx context.Context, ownerID string) ([]*domain.Wallet, error)
	FreezeWallet(ctx context.Context, id string) (*domain.Wallet, error)
	ArchiveWallet(ctx context.Context, id string) (*domain.Wallet, error)
}

// CreateWalletRequest carries the parameters needed to create a new wallet.
type CreateWalletRequest struct {
	ID      string
	OwnerID string
	ChainID domain.ChainID
	Type    domain.WalletType
}

// InMemoryWalletStore stores wallets in a thread-safe map.
type InMemoryWalletStore struct {
	mu      sync.RWMutex
	wallets map[string]*domain.Wallet
}

// NewInMemoryWalletStore constructs an empty InMemoryWalletStore.
func NewInMemoryWalletStore() *InMemoryWalletStore {
	return &InMemoryWalletStore{wallets: make(map[string]*domain.Wallet)}
}

// Save implements WalletStore.
func (s *InMemoryWalletStore) Save(_ context.Context, w *domain.Wallet) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.wallets[w.ID] = w
	return nil
}

// FindByID implements WalletStore.
func (s *InMemoryWalletStore) FindByID(_ context.Context, id string) (*domain.Wallet, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	w, ok := s.wallets[id]
	if !ok {
		return nil, domain.ErrWalletNotFound
	}
	return w, nil
}

// FindByOwner implements WalletStore.
func (s *InMemoryWalletStore) FindByOwner(_ context.Context, ownerID string) ([]*domain.Wallet, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []*domain.Wallet
	for _, w := range s.wallets {
		if w.OwnerID == ownerID {
			out = append(out, w)
		}
	}
	return out, nil
}

// InMemoryWalletService is the in-process implementation of WalletService.
type InMemoryWalletService struct {
	store    WalletStore
	registry *chains.ChainRegistry
}

// NewInMemoryWalletService constructs an InMemoryWalletService.
func NewInMemoryWalletService(store WalletStore, registry *chains.ChainRegistry) *InMemoryWalletService {
	return &InMemoryWalletService{store: store, registry: registry}
}

// CreateWallet implements WalletService.
func (s *InMemoryWalletService) CreateWallet(ctx context.Context, req CreateWalletRequest) (*domain.Wallet, error) {
	if req.ID == "" {
		return nil, fmt.Errorf("wallet ID must not be empty")
	}
	adapter, err := s.registry.Get(req.ChainID)
	if err != nil {
		return nil, domain.ErrChainNotSupported
	}
	addr, err := adapter.GenerateAddress(ctx, req.OwnerID)
	if err != nil {
		return nil, fmt.Errorf("generate address: %w", err)
	}
	w := &domain.Wallet{
		ID:      req.ID,
		OwnerID: req.OwnerID,
		ChainID: req.ChainID,
		Address: addr,
		Type:    req.Type,
		Status:  domain.WalletStatusActive,
	}
	if err := s.store.Save(ctx, w); err != nil {
		return nil, err
	}
	return w, nil
}

// GetWallet implements WalletService.
func (s *InMemoryWalletService) GetWallet(ctx context.Context, id string) (*domain.Wallet, error) {
	return s.store.FindByID(ctx, id)
}

// ListWalletsByOwner implements WalletService.
func (s *InMemoryWalletService) ListWalletsByOwner(ctx context.Context, ownerID string) ([]*domain.Wallet, error) {
	return s.store.FindByOwner(ctx, ownerID)
}

// FreezeWallet marks a wallet as frozen, preventing further use.
func (s *InMemoryWalletService) FreezeWallet(ctx context.Context, id string) (*domain.Wallet, error) {
	w, err := s.store.FindByID(ctx, id)
	if err != nil {
		return nil, err
	}
	if w.Status == domain.WalletStatusArchived {
		return nil, domain.ErrWalletArchived
	}
	w.Status = domain.WalletStatusFrozen
	if err := s.store.Save(ctx, w); err != nil {
		return nil, err
	}
	return w, nil
}

// ArchiveWallet marks a wallet as archived, permanently decommissioning it.
func (s *InMemoryWalletService) ArchiveWallet(ctx context.Context, id string) (*domain.Wallet, error) {
	w, err := s.store.FindByID(ctx, id)
	if err != nil {
		return nil, err
	}
	w.Status = domain.WalletStatusArchived
	if err := s.store.Save(ctx, w); err != nil {
		return nil, err
	}
	return w, nil
}

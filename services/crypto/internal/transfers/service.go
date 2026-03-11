// Package transfers handles internal crypto wallet-to-wallet transfers.
package transfers

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
	"github.com/blazil/crypto/internal/wallets"
)

// InternalTransfer records a completed wallet-to-wallet transfer.
type InternalTransfer struct {
	ID               string
	FromWalletID     string
	ToWalletID       string
	FromAccountID    string
	ToAccountID      string
	ChainID          domain.ChainID
	AmountMinorUnits int64
	CreatedAt        time.Time
}

// InternalTransferRequest carries the parameters for an internal transfer.
type InternalTransferRequest struct {
	ID               string
	FromWalletID     string
	ToWalletID       string
	FromAccountID    string
	ToAccountID      string
	AmountMinorUnits int64
}

// InternalTransferService manages transfers between wallets within the platform.
type InternalTransferService interface {
	Transfer(ctx context.Context, req InternalTransferRequest) (*InternalTransfer, error)
	GetTransfer(ctx context.Context, id string) (*InternalTransfer, error)
}

// InMemoryInternalTransferService implements InternalTransferService.
type InMemoryInternalTransferService struct {
	mu        sync.RWMutex
	transfers map[string]*InternalTransfer
	walletSvc wallets.WalletService
	engine    engine.EngineClient
}

// NewInMemoryInternalTransferService constructs an InMemoryInternalTransferService.
func NewInMemoryInternalTransferService(walletSvc wallets.WalletService, eng engine.EngineClient) *InMemoryInternalTransferService {
	return &InMemoryInternalTransferService{
		transfers: make(map[string]*InternalTransfer),
		walletSvc: walletSvc,
		engine:    eng,
	}
}

// Transfer validates that both wallets exist and are on the same chain, then
// performs an atomic engine debit/credit pair.
func (s *InMemoryInternalTransferService) Transfer(ctx context.Context, req InternalTransferRequest) (*InternalTransfer, error) {
	if req.AmountMinorUnits <= 0 {
		return nil, domain.ErrInsufficientAmount
	}

	from, err := s.walletSvc.GetWallet(ctx, req.FromWalletID)
	if err != nil {
		return nil, fmt.Errorf("from wallet: %w", err)
	}
	to, err := s.walletSvc.GetWallet(ctx, req.ToWalletID)
	if err != nil {
		return nil, fmt.Errorf("to wallet: %w", err)
	}
	if from.Status == domain.WalletStatusFrozen {
		return nil, domain.ErrWalletFrozen
	}
	if from.Status == domain.WalletStatusArchived {
		return nil, domain.ErrWalletArchived
	}
	if from.ChainID != to.ChainID {
		return nil, domain.ErrChainMismatch
	}

	if err := s.engine.Debit(ctx, req.FromAccountID, req.AmountMinorUnits); err != nil {
		return nil, err
	}
	if err := s.engine.Credit(ctx, req.ToAccountID, req.AmountMinorUnits); err != nil {
		// Attempt to refund on credit failure.
		_ = s.engine.Credit(ctx, req.FromAccountID, req.AmountMinorUnits)
		return nil, err
	}

	t := &InternalTransfer{
		ID:               req.ID,
		FromWalletID:     req.FromWalletID,
		ToWalletID:       req.ToWalletID,
		FromAccountID:    req.FromAccountID,
		ToAccountID:      req.ToAccountID,
		ChainID:          from.ChainID,
		AmountMinorUnits: req.AmountMinorUnits,
		CreatedAt:        time.Now().UTC(),
	}
	s.mu.Lock()
	s.transfers[t.ID] = t
	s.mu.Unlock()
	return t, nil
}

// GetTransfer implements InternalTransferService.
func (s *InMemoryInternalTransferService) GetTransfer(_ context.Context, id string) (*InternalTransfer, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	t, ok := s.transfers[id]
	if !ok {
		return nil, fmt.Errorf("transfer not found: %s", id)
	}
	return t, nil
}

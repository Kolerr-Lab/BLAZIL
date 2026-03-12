// Package withdrawals handles outbound on-chain transfers.
package withdrawals

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/observability"
	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
)

// WithdrawalStore is the persistence interface for withdrawals.
type WithdrawalStore interface {
	Save(ctx context.Context, w *domain.Withdrawal) error
	FindByID(ctx context.Context, id string) (*domain.Withdrawal, error)
}

// InMemoryWithdrawalStore stores withdrawals in a thread-safe map.
type InMemoryWithdrawalStore struct {
	mu          sync.RWMutex
	withdrawals map[string]*domain.Withdrawal
}

// NewInMemoryWithdrawalStore constructs an empty InMemoryWithdrawalStore.
func NewInMemoryWithdrawalStore() *InMemoryWithdrawalStore {
	return &InMemoryWithdrawalStore{withdrawals: make(map[string]*domain.Withdrawal)}
}

// Save implements WithdrawalStore.
func (s *InMemoryWithdrawalStore) Save(_ context.Context, w *domain.Withdrawal) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.withdrawals[w.ID] = w
	return nil
}

// FindByID implements WithdrawalStore.
func (s *InMemoryWithdrawalStore) FindByID(_ context.Context, id string) (*domain.Withdrawal, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	w, ok := s.withdrawals[id]
	if !ok {
		return nil, domain.ErrWithdrawalNotFound
	}
	return w, nil
}

// WithdrawalService manages the full lifecycle of a withdrawal request.
type WithdrawalService interface {
	RequestWithdrawal(ctx context.Context, req RequestWithdrawalRequest) (*domain.Withdrawal, error)
	ProcessWithdrawal(ctx context.Context, id string) (*domain.Withdrawal, error)
	GetWithdrawal(ctx context.Context, id string) (*domain.Withdrawal, error)
}

// RequestWithdrawalRequest carries the parameters for requesting a withdrawal.
type RequestWithdrawalRequest struct {
	ID               string
	WalletID         string
	AccountID        string
	ToAddress        string
	ChainID          domain.ChainID
	AmountMinorUnits int64
}

// InMemoryWithdrawalService implements WithdrawalService.
type InMemoryWithdrawalService struct {
	store    WithdrawalStore
	registry *chains.ChainRegistry
	engine   engine.EngineClient
}

// NewInMemoryWithdrawalService constructs an InMemoryWithdrawalService.
func NewInMemoryWithdrawalService(store WithdrawalStore, registry *chains.ChainRegistry, eng engine.EngineClient) *InMemoryWithdrawalService {
	return &InMemoryWithdrawalService{store: store, registry: registry, engine: eng}
}

// RequestWithdrawal validates the request, estimates fees, and persists a
// pending withdrawal record.
func (s *InMemoryWithdrawalService) RequestWithdrawal(ctx context.Context, req RequestWithdrawalRequest) (*domain.Withdrawal, error) {
	if req.AmountMinorUnits <= 0 {
		return nil, domain.ErrInsufficientAmount
	}
	adapter, err := s.registry.Get(req.ChainID)
	if err != nil {
		return nil, domain.ErrChainNotSupported
	}
	fee, err := adapter.EstimateFee(ctx, req.AmountMinorUnits)
	if err != nil {
		return nil, fmt.Errorf("estimate fee: %w", err)
	}
	if req.AmountMinorUnits <= fee {
		return nil, domain.ErrAmountBelowFee
	}
	w := &domain.Withdrawal{
		ID:               req.ID,
		WalletID:         req.WalletID,
		AccountID:        req.AccountID,
		ToAddress:        req.ToAddress,
		ChainID:          req.ChainID,
		AmountMinorUnits: req.AmountMinorUnits,
		FeeMinorUnits:    fee,
		Status:           domain.WithdrawalStatusPending,
		CreatedAt:        time.Now().UTC(),
	}
	if err := s.store.Save(ctx, w); err != nil {
		return nil, err
	}
	return w, nil
}

// ProcessWithdrawal debits the engine account and broadcasts the transaction.
// On broadcast failure the engine account is refunded via Credit.
func (s *InMemoryWithdrawalService) ProcessWithdrawal(ctx context.Context, id string) (*domain.Withdrawal, error) {
	w, err := s.store.FindByID(ctx, id)
	if err != nil {
		return nil, err
	}
	if w.Status != domain.WithdrawalStatusPending {
		return nil, domain.ErrWithdrawalNotPending
	}

	// Resolve chain symbol for metrics labels.
	chainSymbol := fmt.Sprintf("chain-%d", w.ChainID)
	for _, c := range domain.SupportedChains() {
		if c.ID == w.ChainID {
			chainSymbol = c.Symbol
			break
		}
	}

	adapter, err := s.registry.Get(w.ChainID)
	if err != nil {
		return nil, domain.ErrChainNotFound
	}

	// Debit the customer account before broadcasting.
	if err := s.engine.Debit(ctx, w.AccountID, w.AmountMinorUnits); err != nil {
		w.Status = domain.WithdrawalStatusFailed
		_ = s.store.Save(ctx, w)
		observability.WithdrawalsTotal.WithLabelValues(chainSymbol, "failed").Inc()
		return w, err
	}

	txHash, err := adapter.BroadcastTx(ctx, w)
	if err != nil {
		// Refund the debit on broadcast failure.
		_ = s.engine.Credit(ctx, w.AccountID, w.AmountMinorUnits)
		w.Status = domain.WithdrawalStatusFailed
		_ = s.store.Save(ctx, w)
		observability.WithdrawalsTotal.WithLabelValues(chainSymbol, "failed").Inc()
		return w, fmt.Errorf("broadcast: %w", err)
	}

	w.TxHash = txHash
	w.Status = domain.WithdrawalStatusBroadcast
	if err := s.store.Save(ctx, w); err != nil {
		return nil, err
	}
	observability.WithdrawalsTotal.WithLabelValues(chainSymbol, "broadcast").Inc()
	return w, nil
}

// GetWithdrawal implements WithdrawalService.
func (s *InMemoryWithdrawalService) GetWithdrawal(ctx context.Context, id string) (*domain.Withdrawal, error) {
	return s.store.FindByID(ctx, id)
}

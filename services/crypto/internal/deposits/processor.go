// Package deposits handles inbound on-chain transfers.
package deposits

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
	"github.com/blazil/observability"
)

// DepositStore is the persistence interface for deposits.
type DepositStore interface {
	Save(ctx context.Context, d *domain.Deposit) error
	FindByID(ctx context.Context, id string) (*domain.Deposit, error)
	FindByTxHash(ctx context.Context, txHash string) (*domain.Deposit, error)
}

// InMemoryDepositStore stores deposits in a thread-safe map.
type InMemoryDepositStore struct {
	mu       sync.RWMutex
	deposits map[string]*domain.Deposit
}

// NewInMemoryDepositStore constructs an empty InMemoryDepositStore.
func NewInMemoryDepositStore() *InMemoryDepositStore {
	return &InMemoryDepositStore{deposits: make(map[string]*domain.Deposit)}
}

// Save implements DepositStore.
func (s *InMemoryDepositStore) Save(_ context.Context, d *domain.Deposit) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.deposits[d.ID] = d
	return nil
}

// FindByID implements DepositStore.
func (s *InMemoryDepositStore) FindByID(_ context.Context, id string) (*domain.Deposit, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	d, ok := s.deposits[id]
	if !ok {
		return nil, domain.ErrDepositNotFound
	}
	return d, nil
}

// FindByTxHash implements DepositStore.
func (s *InMemoryDepositStore) FindByTxHash(_ context.Context, txHash string) (*domain.Deposit, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, d := range s.deposits {
		if d.TxHash == txHash {
			return d, nil
		}
	}
	return nil, domain.ErrDepositNotFound
}

// DepositDetector registers new on-chain deposits.
type DepositDetector interface {
	Detect(ctx context.Context, req DetectDepositRequest) (*domain.Deposit, error)
}

// DetectDepositRequest carries details of a newly observed inbound transaction.
type DetectDepositRequest struct {
	DepositID        string
	WalletID         string
	AccountID        string
	TxHash           string
	ChainID          domain.ChainID
	AmountMinorUnits int64
}

// InMemoryDepositDetector implements DepositDetector.
type InMemoryDepositDetector struct {
	store DepositStore
}

// NewInMemoryDepositDetector constructs an InMemoryDepositDetector.
func NewInMemoryDepositDetector(store DepositStore) *InMemoryDepositDetector {
	return &InMemoryDepositDetector{store: store}
}

// Detect implements DepositDetector.
func (d *InMemoryDepositDetector) Detect(ctx context.Context, req DetectDepositRequest) (*domain.Deposit, error) {
	if req.AmountMinorUnits <= 0 {
		return nil, domain.ErrInsufficientAmount
	}
	dep := &domain.Deposit{
		ID:               req.DepositID,
		WalletID:         req.WalletID,
		AccountID:        req.AccountID,
		TxHash:           req.TxHash,
		ChainID:          req.ChainID,
		AmountMinorUnits: req.AmountMinorUnits,
		Status:           domain.DepositStatusDetected,
		CreatedAt:        time.Now().UTC(),
	}
	if err := d.store.Save(ctx, dep); err != nil {
		return nil, err
	}
	return dep, nil
}

// DepositProcessor finalizes detected deposits by checking confirmations and
// crediting the customer's engine account.
type DepositProcessor struct {
	store    DepositStore
	registry *chains.ChainRegistry
	engine   engine.EngineClient
}

// NewDepositProcessor constructs a DepositProcessor.
func NewDepositProcessor(store DepositStore, registry *chains.ChainRegistry, eng engine.EngineClient) *DepositProcessor {
	return &DepositProcessor{store: store, registry: registry, engine: eng}
}

// Process checks confirmations for a detected deposit and, if sufficient,
// credits the engine account and marks the deposit processed.
func (p *DepositProcessor) Process(ctx context.Context, depositID string) (*domain.Deposit, error) {
	dep, err := p.store.FindByID(ctx, depositID)
	if err != nil {
		return nil, err
	}
	if dep.Status == domain.DepositStatusProcessed {
		return nil, domain.ErrDepositAlreadyProcessed
	}

	adapter, err := p.registry.Get(dep.ChainID)
	if err != nil {
		return nil, domain.ErrChainNotFound
	}

	// Resolve required confirmations from supported chains list.
	requiredConfs := 0
	chainSymbol := fmt.Sprintf("chain-%d", dep.ChainID)
	for _, c := range domain.SupportedChains() {
		if c.ID == dep.ChainID {
			requiredConfs = c.RequiredConfirmations
			chainSymbol = c.Symbol
			break
		}
	}

	confs, err := adapter.GetConfirmations(ctx, dep.TxHash)
	if err != nil {
		return nil, fmt.Errorf("get confirmations: %w", err)
	}
	dep.Confirmations = confs

	if confs < requiredConfs {
		dep.Status = domain.DepositStatusDetected
		_ = p.store.Save(ctx, dep)
		return nil, domain.ErrNotEnoughConfirmations
	}

	// Credit the customer's engine account.
	if err := p.engine.Credit(ctx, dep.AccountID, dep.AmountMinorUnits); err != nil {
		dep.Status = domain.DepositStatusFailed
		_ = p.store.Save(ctx, dep)
		observability.DepositsTotal.WithLabelValues(chainSymbol, "failed").Inc()
		return dep, err
	}

	now := time.Now().UTC()
	dep.Status = domain.DepositStatusProcessed
	dep.ProcessedAt = &now
	if err := p.store.Save(ctx, dep); err != nil {
		return nil, err
	}
	observability.DepositsTotal.WithLabelValues(chainSymbol, "processed").Inc()
	return dep, nil
}

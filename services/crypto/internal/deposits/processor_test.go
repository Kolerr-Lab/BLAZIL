package deposits_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/deposits"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
)

func newRegistry() *chains.ChainRegistry {
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		reg.Register(chains.NewMockChainAdapter(c))
	}
	return reg
}

func TestDetect_Success(t *testing.T) {
	store := deposits.NewInMemoryDepositStore()
	detector := deposits.NewInMemoryDepositDetector(store)
	dep, err := detector.Detect(context.Background(), deposits.DetectDepositRequest{
		DepositID:        "dep-1",
		WalletID:         "w1",
		AccountID:        "acc-1",
		TxHash:           "0xabc",
		ChainID:          domain.ChainEthereum,
		AmountMinorUnits: 500000,
	})
	if err != nil {
		t.Fatalf("Detect: %v", err)
	}
	if dep.Status != domain.DepositStatusDetected {
		t.Errorf("expected detected, got %s", dep.Status)
	}
}

func TestProcess_Success(t *testing.T) {
	store := deposits.NewInMemoryDepositStore()
	detector := deposits.NewInMemoryDepositDetector(store)
	eng := &engine.MockEngineClient{}
	processor := deposits.NewDepositProcessor(store, newRegistry(), eng)

	ctx := context.Background()
	_, _ = detector.Detect(ctx, deposits.DetectDepositRequest{
		DepositID:        "dep-2",
		WalletID:         "w1",
		AccountID:        "acc-1",
		TxHash:           "0xdef",
		ChainID:          domain.ChainEthereum,
		AmountMinorUnits: 1000000,
	})

	dep, err := processor.Process(ctx, "dep-2")
	if err != nil {
		t.Fatalf("Process: %v", err)
	}
	if dep.Status != domain.DepositStatusProcessed {
		t.Errorf("expected processed, got %s", dep.Status)
	}
	if len(eng.Calls) == 0 {
		t.Error("expected engine Credit to be called")
	}
}

func TestProcess_AlreadyProcessed(t *testing.T) {
	store := deposits.NewInMemoryDepositStore()
	detector := deposits.NewInMemoryDepositDetector(store)
	eng := &engine.MockEngineClient{}
	processor := deposits.NewDepositProcessor(store, newRegistry(), eng)

	ctx := context.Background()
	_, _ = detector.Detect(ctx, deposits.DetectDepositRequest{
		DepositID: "dep-3", WalletID: "w1", AccountID: "acc-1",
		TxHash: "0xfed", ChainID: domain.ChainBitcoin, AmountMinorUnits: 50000,
	})
	_, _ = processor.Process(ctx, "dep-3")
	_, err := processor.Process(ctx, "dep-3")
	if !errors.Is(err, domain.ErrDepositAlreadyProcessed) {
		t.Errorf("expected ErrDepositAlreadyProcessed, got %v", err)
	}
}

// notEnoughConfsAdapter wraps MockChainAdapter but returns 0 confirmations.
type notEnoughConfsAdapter struct {
	inner *chains.MockChainAdapter
}

func (a *notEnoughConfsAdapter) ChainID() domain.ChainID { return a.inner.ChainID() }
func (a *notEnoughConfsAdapter) GenerateAddress(ctx context.Context, ownerID string) (string, error) {
	return a.inner.GenerateAddress(ctx, ownerID)
}
func (a *notEnoughConfsAdapter) EstimateFee(ctx context.Context, amt int64) (int64, error) {
	return a.inner.EstimateFee(ctx, amt)
}
func (a *notEnoughConfsAdapter) BroadcastTx(ctx context.Context, w *domain.Withdrawal) (string, error) {
	return a.inner.BroadcastTx(ctx, w)
}
func (a *notEnoughConfsAdapter) GetConfirmations(_ context.Context, _ string) (int, error) {
	return 0, nil
}

func TestProcess_NotEnoughConfirmations(t *testing.T) {
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		if c.ID == domain.ChainEthereum {
			reg.Register(&notEnoughConfsAdapter{inner: chains.NewMockChainAdapter(c)})
		} else {
			reg.Register(chains.NewMockChainAdapter(c))
		}
	}
	store := deposits.NewInMemoryDepositStore()
	detector := deposits.NewInMemoryDepositDetector(store)
	eng := &engine.MockEngineClient{}
	processor := deposits.NewDepositProcessor(store, reg, eng)

	ctx := context.Background()
	_, _ = detector.Detect(ctx, deposits.DetectDepositRequest{
		DepositID: "dep-4", WalletID: "w1", AccountID: "acc-1",
		TxHash: "0x000", ChainID: domain.ChainEthereum, AmountMinorUnits: 200000,
	})
	_, err := processor.Process(ctx, "dep-4")
	if !errors.Is(err, domain.ErrNotEnoughConfirmations) {
		t.Errorf("expected ErrNotEnoughConfirmations, got %v", err)
	}
}

func TestProcess_EngineFailure_StatusFailed(t *testing.T) {
	store := deposits.NewInMemoryDepositStore()
	detector := deposits.NewInMemoryDepositDetector(store)
	eng := &engine.MockEngineClient{CreditErr: errors.New("engine down")}
	processor := deposits.NewDepositProcessor(store, newRegistry(), eng)

	ctx := context.Background()
	_, _ = detector.Detect(ctx, deposits.DetectDepositRequest{
		DepositID: "dep-5", WalletID: "w1", AccountID: "acc-1",
		TxHash: "0x999", ChainID: domain.ChainBitcoin, AmountMinorUnits: 100000,
	})
	dep, err := processor.Process(ctx, "dep-5")
	if err == nil {
		t.Fatal("expected error from engine failure")
	}
	if dep == nil || dep.Status != domain.DepositStatusFailed {
		t.Errorf("expected deposit status failed, got %v", dep)
	}
}

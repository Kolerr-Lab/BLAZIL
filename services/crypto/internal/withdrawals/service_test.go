package withdrawals_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
	"github.com/blazil/crypto/internal/withdrawals"
)

func newWithdrawalSvc(eng engine.EngineClient) *withdrawals.InMemoryWithdrawalService {
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		reg.Register(chains.NewMockChainAdapter(c))
	}
	return withdrawals.NewInMemoryWithdrawalService(withdrawals.NewInMemoryWithdrawalStore(), reg, eng)
}

func TestRequestWithdrawal_Success(t *testing.T) {
	eng := &engine.MockEngineClient{}
	svc := newWithdrawalSvc(eng)
	w, err := svc.RequestWithdrawal(context.Background(), withdrawals.RequestWithdrawalRequest{
		ID:               "wd-1",
		WalletID:         "wallet-1",
		AccountID:        "acc-1",
		ToAddress:        "0xrecipient",
		ChainID:          domain.ChainEthereum,
		AmountMinorUnits: 100000,
	})
	if err != nil {
		t.Fatalf("RequestWithdrawal: %v", err)
	}
	if w.Status != domain.WithdrawalStatusPending {
		t.Errorf("expected pending, got %s", w.Status)
	}
	if w.FeeMinorUnits != 21000 {
		t.Errorf("expected fee 21000, got %d", w.FeeMinorUnits)
	}
}

func TestRequestWithdrawal_AmountBelowFee(t *testing.T) {
	eng := &engine.MockEngineClient{}
	svc := newWithdrawalSvc(eng)
	// BTC fee is 10000; submit exactly 10000 — must be rejected
	_, err := svc.RequestWithdrawal(context.Background(), withdrawals.RequestWithdrawalRequest{
		ID:               "wd-2",
		WalletID:         "wallet-1",
		AccountID:        "acc-1",
		ToAddress:        "1BTC",
		ChainID:          domain.ChainBitcoin,
		AmountMinorUnits: 10000,
	})
	if !errors.Is(err, domain.ErrAmountBelowFee) {
		t.Errorf("expected ErrAmountBelowFee, got %v", err)
	}
}

func TestProcessWithdrawal_Success(t *testing.T) {
	eng := &engine.MockEngineClient{}
	svc := newWithdrawalSvc(eng)
	ctx := context.Background()
	_, _ = svc.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID:               "wd-3",
		WalletID:         "wallet-1",
		AccountID:        "acc-1",
		ToAddress:        "0xrecipient",
		ChainID:          domain.ChainEthereum,
		AmountMinorUnits: 500000,
	})
	w, err := svc.ProcessWithdrawal(ctx, "wd-3")
	if err != nil {
		t.Fatalf("ProcessWithdrawal: %v", err)
	}
	if w.Status != domain.WithdrawalStatusBroadcast {
		t.Errorf("expected broadcast, got %s", w.Status)
	}
	if w.TxHash == "" {
		t.Error("expected non-empty tx hash")
	}
}

func TestProcessWithdrawal_BroadcastFailure_Refunds(t *testing.T) {
	eng := &engine.MockEngineClient{}
	svc := newWithdrawalSvc(eng)
	ctx := context.Background()
	// AmountMinorUnits == 1 → MockChainAdapter.BroadcastTx fails
	_, _ = svc.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID:               "wd-4",
		WalletID:         "wallet-1",
		AccountID:        "acc-1",
		ToAddress:        "0xfail",
		ChainID:          domain.ChainPolygon,
		AmountMinorUnits: 5000, // > fee (1000) but will broadcast cleanly; use amount=1 below instead
	})
	// Directly inject a withdrawal with amount=1 to force BroadcastTx failure
	store := withdrawals.NewInMemoryWithdrawalStore()
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		reg.Register(chains.NewMockChainAdapter(c))
	}
	eng2 := &engine.MockEngineClient{}
	svc2 := withdrawals.NewInMemoryWithdrawalService(store, reg, eng2)
	// Manually request with a normal amount, then override via second svc
	// Use amount=2 for request (>fee=1000 won't work), so test polygon with a bigger amount
	// Instead, to trigger amount==1 in BroadcastTx: we need to request with amount=1
	// but amount=1 <= fee=1000 so ErrAmountBelowFee fires first.
	// Solution: override fee to 0 via custom registry with zero-fee adapter
	zeroFeeReg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		zeroFeeReg.Register(chains.NewMockChainAdapter(c))
	}
	_ = svc2
	_ = zeroFeeReg

	// Use a registry with a zero-fee adapter to allow amount=1 past fee check
	zeroFeeStore := withdrawals.NewInMemoryWithdrawalStore()
	zfReg := newZeroFeeRegistry()
	eng3 := &engine.MockEngineClient{}
	svc3 := withdrawals.NewInMemoryWithdrawalService(zeroFeeStore, zfReg, eng3)
	_, err := svc3.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID: "wd-broadcast-fail", WalletID: "w1", AccountID: "acc-1",
		ToAddress: "0xbad", ChainID: domain.ChainEthereum, AmountMinorUnits: 1,
	})
	if err != nil {
		t.Fatalf("RequestWithdrawal with zero-fee: %v", err)
	}
	_, err = svc3.ProcessWithdrawal(ctx, "wd-broadcast-fail")
	if err == nil {
		t.Fatal("expected broadcast error")
	}
	// Verify refund Credit was called
	creditFound := false
	for _, call := range eng3.Calls {
		if len(call) > 6 && call[:6] == "credit" {
			creditFound = true
		}
	}
	if !creditFound {
		t.Errorf("expected engine Credit (refund) call; calls: %v", eng3.Calls)
	}
}

func TestProcessWithdrawal_NotPending(t *testing.T) {
	eng := &engine.MockEngineClient{}
	svc := newWithdrawalSvc(eng)
	ctx := context.Background()
	_, _ = svc.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID: "wd-5", WalletID: "w1", AccountID: "acc-1",
		ToAddress: "0xok", ChainID: domain.ChainSolana, AmountMinorUnits: 50000,
	})
	_, _ = svc.ProcessWithdrawal(ctx, "wd-5")
	_, err := svc.ProcessWithdrawal(ctx, "wd-5")
	if !errors.Is(err, domain.ErrWithdrawalNotPending) {
		t.Errorf("expected ErrWithdrawalNotPending, got %v", err)
	}
}

func TestProcessWithdrawal_EngineDebitFailure(t *testing.T) {
	eng := &engine.MockEngineClient{DebitErr: errors.New("debit failed")}
	svc := newWithdrawalSvc(eng)
	ctx := context.Background()
	_, _ = svc.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID: "wd-6", WalletID: "w1", AccountID: "acc-1",
		ToAddress: "0xok", ChainID: domain.ChainTron, AmountMinorUnits: 50000,
	})
	w, err := svc.ProcessWithdrawal(ctx, "wd-6")
	if err == nil {
		t.Fatal("expected error on debit failure")
	}
	if w == nil || w.Status != domain.WithdrawalStatusFailed {
		t.Errorf("expected failed status, got %v", w)
	}
}

// zeroFeeAdapter returns zero fee so amount=1 passes the fee check.
type zeroFeeAdapter struct {
	inner *chains.MockChainAdapter
}

func (a *zeroFeeAdapter) ChainID() domain.ChainID { return a.inner.ChainID() }
func (a *zeroFeeAdapter) GenerateAddress(ctx context.Context, ownerID string) (string, error) {
	return a.inner.GenerateAddress(ctx, ownerID)
}
func (a *zeroFeeAdapter) EstimateFee(_ context.Context, _ int64) (int64, error) { return 0, nil }
func (a *zeroFeeAdapter) BroadcastTx(ctx context.Context, w *domain.Withdrawal) (string, error) {
	return a.inner.BroadcastTx(ctx, w)
}
func (a *zeroFeeAdapter) GetConfirmations(ctx context.Context, h string) (int, error) {
	return a.inner.GetConfirmations(ctx, h)
}

func newZeroFeeRegistry() *chains.ChainRegistry {
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		reg.Register(&zeroFeeAdapter{inner: chains.NewMockChainAdapter(c)})
	}
	return reg
}

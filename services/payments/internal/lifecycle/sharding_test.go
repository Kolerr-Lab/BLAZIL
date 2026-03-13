package lifecycle_test

import (
	"context"
	"testing"
	"time"

	"github.com/blazil/sharding"
	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/lifecycle"
	"github.com/blazil/services/payments/internal/routing"
)

// buildProcessorForSharding wires a PaymentProcessor the same way buildProcessor
// does but is defined here to keep the sharding tests self-contained.
func buildProcessorForSharding(mockEngine *engine.MockEngineClient) *lifecycle.PaymentProcessor {
	authCfg := authorization.AuthorizerConfig{
		MaxAmountMinorUnits:  100_000_000_00,
		WarnAmountMinorUnits: 10_000_000_00,
		LedgerCurrencies:     map[domain.LedgerID]string{1: "USD"},
	}
	routerCfg := routing.RouterConfig{
		LocalAccounts: map[domain.AccountID]struct{}{
			"local-A": {},
			"local-B": {},
		},
		ACHLimitMinorUnits: 2_499_999,
	}
	return lifecycle.NewPaymentProcessor(
		lifecycle.NewInMemoryPaymentStore(),
		authorization.NewCompositeAuthorizer(authCfg),
		routing.NewRuleBasedRouter(routerCfg),
		lifecycle.NewInMemoryIdempotencyStore(24*time.Hour),
		mockEngine,
	)
}

// TestPayments_ShardingDisabled_NoChange verifies that a PaymentProcessor with
// no sharding configuration exhibits the same behaviour as before — i.e. zero
// behaviour change when BLAZIL_NODES is not set / sharding is opt-in.
func TestPayments_ShardingDisabled_NoChange(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessorForSharding(mock) // no SetShardRouter → disabled

	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "sharding-disabled-1",
		DebitAccountID:  "ext-X",
		CreditAccountID: "ext-Y",
		Amount:          domain.NewMoney(500, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "REF-001"},
	}

	p, err := proc.Process(context.Background(), req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if p == nil {
		t.Fatal("expected non-nil payment")
	}
	// Status should follow normal lifecycle (ACH external → cleared).
	if p.Status == domain.StatusPending {
		t.Errorf("payment stuck in pending status")
	}
}

// TestPayments_ShardingEnabled_Routes verifies that when a ShardRouter is
// wired into the processor, IsCrossShard is invoked during payment processing.
// This confirms the shard-aware routing code path is exercised.
func TestPayments_ShardingEnabled_Routes(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessorForSharding(mock)

	mockRouter := sharding.NewMockShardRouter()
	proc.SetShardRouter(mockRouter)

	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "sharding-enabled-1",
		DebitAccountID:  "account-X",
		CreditAccountID: "account-Y",
		Amount:          domain.NewMoney(1000, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "REF-002"},
	}

	_, err := proc.Process(context.Background(), req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if mockRouter.IsCrossShardCallCount() == 0 {
		t.Error("expected IsCrossShard to be called at least once during processing")
	}
}

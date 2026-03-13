package lifecycle_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/lifecycle"
	"github.com/blazil/services/payments/internal/routing"
	"github.com/blazil/sharding"
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

// mockCoordinator is a test-local CrossShardCoordinator that records Execute
// calls and returns a configurable error.
type mockCoordinator struct {
	executeCount int
	returnErr    error
}

func (m *mockCoordinator) Execute(_ context.Context, _ sharding.CrossShardRequest) error {
	m.executeCount++
	return m.returnErr
}

// TestPayments_CoordinatorExecutes_CrossShard verifies that when a coordinator
// is wired and IsCrossShard returns true, the coordinator's Execute method is
// called (and the normal engine path is skipped).
func TestPayments_CoordinatorExecutes_CrossShard(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessorForSharding(mock)

	// Use a real MockShardRouter so IsCrossShard uses actual jump hash.
	// Pick two account IDs that jump-hash to different shards (3 shards).
	// account-X and account-Z are chosen so IsCrossShard returns true;
	// the test asserts coordinator.Execute is invoked.
	mockRouter := sharding.NewMockShardRouter()
	coordinator := &mockCoordinator{}
	proc.SetShardRouter(mockRouter)
	proc.SetCrossShardCoordinator(coordinator)

	// Find the first cross-shard pair by iterating until IsCrossShard is true.
	var debitID, creditID string
	for i := 0; i < 1000; i++ {
		for j := i + 1; j < 1000; j++ {
			a := domain.AccountID(fmt.Sprintf("account-%d", i))
			b := domain.AccountID(fmt.Sprintf("account-%d", j))
			if mockRouter.IsCrossShard(hashAccount(a), hashAccount(b)) {
				debitID = string(a)
				creditID = string(b)
				goto found
			}
		}
	}
found:
	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "coordinator-xshard-1",
		DebitAccountID:  domain.AccountID(debitID),
		CreditAccountID: domain.AccountID(creditID),
		Amount:          domain.NewMoney(2500, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "REF-003"},
	}

	_, err := proc.Process(context.Background(), req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if coordinator.executeCount == 0 {
		t.Error("expected coordinator.Execute to be called for cross-shard payment, but it was not")
	}
}

// hashAccount mirrors accountToUint64 used in processor.go so the test can
// compute shard IDs without accessing the unexported helper.
func hashAccount(id domain.AccountID) uint64 {
	var h uint64 = 14695981039346656037 // FNV-1a offset basis
	for _, b := range []byte(id) {
		h ^= uint64(b)
		h *= 1099511628211
	}
	return h
}

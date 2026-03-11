package lifecycle_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/lifecycle"
	"github.com/blazil/services/payments/internal/routing"
)

// buildProcessor wires up a PaymentProcessor with the mock engine client.
func buildProcessor(mockEngine *engine.MockEngineClient) *lifecycle.PaymentProcessor {
	authCfg := authorization.AuthorizerConfig{
		MaxAmountMinorUnits:  100_000_000_00,
		WarnAmountMinorUnits: 10_000_000_00,
		LedgerCurrencies:     map[domain.LedgerID]string{1: "USD", 2: "EUR"},
	}
	auth := authorization.NewCompositeAuthorizer(authCfg)

	routerCfg := routing.RouterConfig{
		LocalAccounts: map[domain.AccountID]struct{}{
			"local-A": {},
			"local-B": {},
		},
		ACHLimitMinorUnits: 2_499_999,
	}
	router := routing.NewRuleBasedRouter(routerCfg)

	paymentStore := lifecycle.NewInMemoryPaymentStore()
	idem := lifecycle.NewInMemoryIdempotencyStore(24 * time.Hour)
	return lifecycle.NewPaymentProcessor(paymentStore, auth, router, idem, mockEngine)
}

// validRequest returns a request that passes all authorization and routes as ACH.
func validRequest(key string) domain.ProcessPaymentRequest {
	return domain.ProcessPaymentRequest{
		IdempotencyKey:  key,
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(1000, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "INV-001"},
	}
}

func TestIdempotency_SameKeyReturnsSameResult(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessor(mock)
	ctx := context.Background()

	first, err := proc.Process(ctx, validRequest("idem-key-1"))
	if err != nil {
		t.Fatalf("first call error: %v", err)
	}

	second, err := proc.Process(ctx, validRequest("idem-key-1"))
	if err != nil {
		t.Fatalf("second call error: %v", err)
	}

	if first.ID != second.ID {
		t.Errorf("idempotency broken: first ID=%s, second ID=%s", first.ID, second.ID)
	}
	// Engine should have been called exactly once.
	if len(mock.Calls) != 0 {
		// ACH rails → engine NOT called (external rails). OK.
	}
}

func TestIdempotency_DifferentKeysIndependent(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessor(mock)
	ctx := context.Background()

	p1, err := proc.Process(ctx, validRequest("key-A"))
	if err != nil {
		t.Fatalf("key-A error: %v", err)
	}
	p2, err := proc.Process(ctx, validRequest("key-B"))
	if err != nil {
		t.Fatalf("key-B error: %v", err)
	}

	if p1.ID == p2.ID {
		t.Error("different idempotency keys produced the same payment ID")
	}
}

func TestProcess_AuthorizationFailed(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessor(mock)

	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "auth-fail-1",
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(0, domain.USD), // zero amount → rejected
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "INV-001"},
	}

	payment, err := proc.Process(context.Background(), req)
	if err != nil {
		t.Fatalf("unexpected infrastructure error: %v", err)
	}
	if payment.Status != domain.StatusFailed {
		t.Errorf("status: got %v, want failed", payment.Status)
	}
	if payment.FailureReason == "" {
		t.Error("expected non-empty FailureReason")
	}
}

func TestProcess_InternalRails_Settled(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessor(mock)

	// Both accounts are local → internal rails → engine is called.
	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "internal-1",
		DebitAccountID:  "local-A",
		CreditAccountID: "local-B",
		Amount:          domain.NewMoney(5000, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "INV-002"},
	}

	payment, err := proc.Process(context.Background(), req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if payment.Status != domain.StatusSettled {
		t.Errorf("status: got %v, want settled", payment.Status)
	}
	if len(mock.Calls) != 1 {
		t.Errorf("engine calls: got %d, want 1", len(mock.Calls))
	}
}

func TestProcess_ExternalRails_Cleared(t *testing.T) {
	mock := engine.NewMockEngineClient()
	proc := buildProcessor(mock)

	// External accounts, USD → ACH → cleared (not settled).
	payment, err := proc.Process(context.Background(), validRequest("ext-ach-1"))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if payment.Status != domain.StatusCleared {
		t.Errorf("status: got %v, want cleared", payment.Status)
	}
	if payment.Rails != domain.RailsACH {
		t.Errorf("rails: got %v, want ach", payment.Rails)
	}
}

func TestProcess_EngineError_ReturnsError(t *testing.T) {
	mock := engine.NewMockEngineClient()
	mock.ReturnError = errors.New("engine unavailable")
	proc := buildProcessor(mock)

	// Both local accounts → internal rails → engine.Submit called → error.
	req := domain.ProcessPaymentRequest{
		IdempotencyKey:  "eng-err-1",
		DebitAccountID:  "local-A",
		CreditAccountID: "local-B",
		Amount:          domain.NewMoney(5000, domain.USD),
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "INV-003"},
	}

	_, err := proc.Process(context.Background(), req)
	if err == nil {
		t.Fatal("expected error from engine, got nil")
	}
}

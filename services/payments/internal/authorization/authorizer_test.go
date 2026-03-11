package authorization_test

import (
	"context"
	"testing"

	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/domain"
)

// cfg is the standard test configuration.
var cfg = authorization.AuthorizerConfig{
	MaxAmountMinorUnits:  100_000_000_00, // $1,000,000.00
	WarnAmountMinorUnits: 10_000_000_00,  // $100,000.00 (warn threshold)
	LedgerCurrencies: map[domain.LedgerID]string{
		1: "USD",
		2: "EUR",
		6: "BTC",
		7: "ETH",
	},
}

// validPayment returns a payment that passes all rules.
func validPayment() *domain.Payment {
	return &domain.Payment{
		DebitAccountID:  "account-A",
		CreditAccountID: "account-B",
		Amount:          domain.NewMoney(1000, domain.USD), // $10.00
		LedgerID:        1,
		Metadata:        map[string]string{"reference": "INV-001"},
	}
}

func TestZeroAmountRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	p.Amount = domain.NewMoney(0, domain.USD)

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for zero amount")
	}
	if result.Reason != "payment amount must be positive" {
		t.Errorf("unexpected reason: %q", result.Reason)
	}
}

func TestNegativeAmountRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	p.Amount = domain.NewMoney(-100, domain.USD)

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for negative amount")
	}
}

func TestSameAccountRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	p.CreditAccountID = p.DebitAccountID

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for same debit/credit account")
	}
	if result.Reason != "debit and credit accounts must differ" {
		t.Errorf("unexpected reason: %q", result.Reason)
	}
}

func TestCurrencyMismatchRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	// USD amount on the EUR ledger (ledger_id=2).
	p.Amount = domain.NewMoney(1000, domain.USD)
	p.LedgerID = 2

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for currency/ledger mismatch")
	}
	if result.Reason != "currency mismatch for ledger" {
		t.Errorf("unexpected reason: %q", result.Reason)
	}
}

func TestAmountExceedsLimitRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	p.Amount = domain.NewMoney(200_000_000_00, domain.USD) // $2,000,000 > $1,000,000 limit

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for amount exceeding limit")
	}
	if result.Reason != "amount exceeds single-payment limit" {
		t.Errorf("unexpected reason: %q", result.Reason)
	}
}

func TestMissingReferenceRejected(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	p := validPayment()
	p.Metadata = map[string]string{"other": "value"} // no "reference" key

	result := auth.Authorize(context.Background(), p)
	if result.Approved {
		t.Fatal("expected rejection for missing reference metadata")
	}
	if result.Reason != "payment reference is required" {
		t.Errorf("unexpected reason: %q", result.Reason)
	}
}

func TestValidPaymentApproved(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)
	result := auth.Authorize(context.Background(), validPayment())
	if !result.Approved {
		t.Fatalf("expected approval; reason: %s", result.Reason)
	}
}

func TestRiskScoreAccumulates(t *testing.T) {
	auth := authorization.NewCompositeAuthorizer(cfg)

	// BTC crypto payment → currencyConsistencyRule warns (+10).
	// Amount > WarnAmountMinorUnits threshold (1B) → maxAmountRule warns (+10).
	// Expected total: 2 warnings × 10 = 20.
	btcHighAmount := domain.NewMoney(2_000_000_000, domain.BTC) // 20 BTC (>1B satoshi warn threshold)
	p := &domain.Payment{
		DebitAccountID:  "account-A",
		CreditAccountID: "account-B",
		Amount:          btcHighAmount,
		LedgerID:        6, // BTC ledger
		Metadata:        map[string]string{"reference": "INV-BTC-001"},
	}

	result := auth.Authorize(context.Background(), p)
	if !result.Approved {
		t.Fatalf("expected approval; reason: %s", result.Reason)
	}
	if result.RiskScore != 20 {
		t.Errorf("RiskScore: got %d, want 20", result.RiskScore)
	}
}

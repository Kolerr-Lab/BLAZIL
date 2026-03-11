package routing_test

import (
	"context"
	"testing"

	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/routing"
)

func localAccountsCfg(ids ...domain.AccountID) routing.RouterConfig {
	cfg := routing.DefaultRouterConfig()
	for _, id := range ids {
		cfg.LocalAccounts[id] = struct{}{}
	}
	return cfg
}

func TestInternalRouting(t *testing.T) {
	cfg := localAccountsCfg("account-A", "account-B")
	router := routing.NewRuleBasedRouter(cfg)

	p := &domain.Payment{
		DebitAccountID:  "account-A",
		CreditAccountID: "account-B",
		Amount:          domain.NewMoney(1000, domain.USD),
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsInternal {
		t.Errorf("rails: got %v, want internal", rails)
	}
}

func TestACHRouting_USDUnderLimit(t *testing.T) {
	router := routing.NewRuleBasedRouter(routing.DefaultRouterConfig())

	p := &domain.Payment{
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(100_00, domain.USD), // $100 USD
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsACH {
		t.Errorf("rails: got %v, want ach", rails)
	}
}

func TestSEPARouting_EUR(t *testing.T) {
	router := routing.NewRuleBasedRouter(routing.DefaultRouterConfig())

	p := &domain.Payment{
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(5000, domain.EUR),
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsSEPA {
		t.Errorf("rails: got %v, want sepa", rails)
	}
}

func TestCryptoRouting_BTC(t *testing.T) {
	router := routing.NewRuleBasedRouter(routing.DefaultRouterConfig())

	p := &domain.Payment{
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(1_000_000, domain.BTC),
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsCrypto {
		t.Errorf("rails: got %v, want crypto", rails)
	}
}

func TestCryptoRouting_ETH(t *testing.T) {
	router := routing.NewRuleBasedRouter(routing.DefaultRouterConfig())

	p := &domain.Payment{
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(1_000_000, domain.ETH),
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsCrypto {
		t.Errorf("rails: got %v, want crypto", rails)
	}
}

func TestSWIFTRouting_Default(t *testing.T) {
	router := routing.NewRuleBasedRouter(routing.DefaultRouterConfig())

	// GBP is not handled by internal, ACH, SEPA, or crypto rules → SWIFT.
	p := &domain.Payment{
		DebitAccountID:  "ext-A",
		CreditAccountID: "ext-B",
		Amount:          domain.NewMoney(10000, domain.GBP),
	}

	rails, err := router.Route(context.Background(), p)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if rails != domain.RailsSWIFT {
		t.Errorf("rails: got %v, want swift", rails)
	}
}

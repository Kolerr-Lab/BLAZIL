// Package authorization implements the payment authorization engine.
package authorization

import (
	"context"

	"github.com/blazil/services/payments/internal/domain"
)

// AuthorizationResult is the outcome of running an authorization check.
type AuthorizationResult struct {
	// Approved is true when all rules passed (possibly with warnings).
	Approved bool

	// Reason contains a human-readable rejection message when Approved is false,
	// or a summary when Approved is true.
	Reason string

	// RiskScore is a value from 0–100 indicating the aggregate risk level.
	// Computed as: number of warning-level rule triggers × 10.
	RiskScore int
}

// AuthorizerConfig holds configurable parameters for the authorization engine.
type AuthorizerConfig struct {
	// MaxAmountMinorUnits is the maximum allowed payment amount in minor units.
	// Default equivalent: 1,000,000 USD = 100_000_000 cents.
	MaxAmountMinorUnits int64

	// WarnAmountMinorUnits is the threshold above which a risk warning is added
	// (but the payment is not rejected). Defaults to 10% of MaxAmountMinorUnits.
	WarnAmountMinorUnits int64

	// LedgerCurrencies maps a LedgerID to its expected currency code.
	// Example: {1: "USD", 2: "EUR"}.
	LedgerCurrencies map[domain.LedgerID]string
}

// DefaultAuthorizerConfig returns a sensible default configuration.
func DefaultAuthorizerConfig() AuthorizerConfig {
	return AuthorizerConfig{
		MaxAmountMinorUnits:  100_000_000_00, // $1,000,000.00 USD in cents
		WarnAmountMinorUnits: 10_000_000_00,  // $100,000.00 USD warning threshold
		LedgerCurrencies: map[domain.LedgerID]string{
			1: "USD",
			2: "EUR",
			3: "GBP",
			4: "JPY",
			5: "VND",
			6: "BTC",
			7: "ETH",
		},
	}
}

// Authorizer is the interface implemented by any authorization engine.
type Authorizer interface {
	// Authorize evaluates the payment against all configured rules.
	Authorize(ctx context.Context, p *domain.Payment) AuthorizationResult
}

// authorizationRule is a single authorization check, internal to this package.
type authorizationRule interface {
	// evaluate returns: approved, warned, reason.
	evaluate(ctx context.Context, p *domain.Payment, cfg AuthorizerConfig) (approved bool, warned bool, reason string)
}

// CompositeAuthorizer runs all rules in order, returning the first rejection or
// a final approval with accumulated risk score.
type CompositeAuthorizer struct {
	rules []authorizationRule
	cfg   AuthorizerConfig
}

// NewCompositeAuthorizer constructs a CompositeAuthorizer with the standard
// built-in rule set.
func NewCompositeAuthorizer(cfg AuthorizerConfig) *CompositeAuthorizer {
	return &CompositeAuthorizer{
		cfg: cfg,
		rules: []authorizationRule{
			&zeroAmountRule{},
			&sameAccountRule{},
			&currencyConsistencyRule{},
			&maxAmountRule{},
			&metadataRule{},
		},
	}
}

// Authorize implements Authorizer.
func (a *CompositeAuthorizer) Authorize(ctx context.Context, p *domain.Payment) AuthorizationResult {
	warnings := 0

	for _, rule := range a.rules {
		approved, warned, reason := rule.evaluate(ctx, p, a.cfg)
		if !approved {
			return AuthorizationResult{
				Approved:  false,
				Reason:    reason,
				RiskScore: (warnings + 1) * 10,
			}
		}
		if warned {
			warnings++
		}
	}

	return AuthorizationResult{
		Approved:  true,
		Reason:    "all checks passed",
		RiskScore: warnings * 10,
	}
}

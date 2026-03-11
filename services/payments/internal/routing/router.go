// Package routing implements payment rail selection logic.
package routing

import (
	"context"
	"fmt"

	"github.com/blazil/services/payments/internal/domain"
)

// PaymentRouter selects the appropriate payment rail for a given payment.
type PaymentRouter interface {
	// Route returns the PaymentRails that should be used to process this payment.
	Route(ctx context.Context, p *domain.Payment) (domain.PaymentRails, error)
}

// RouterConfig holds configuration for the rule-based router.
type RouterConfig struct {
	// LocalAccounts is the set of account IDs that are local to this Blazil instance.
	// A payment is routed internally only when BOTH debit and credit accounts are local.
	LocalAccounts map[domain.AccountID]struct{}

	// ACHLimitMinorUnits is the maximum USD amount (in cents) eligible for ACH routing.
	// Defaults to $24,999.99 = 2_499_999 cents (same-day ACH limit).
	ACHLimitMinorUnits int64
}

// DefaultRouterConfig returns a RouterConfig with sensible defaults.
func DefaultRouterConfig() RouterConfig {
	return RouterConfig{
		LocalAccounts:      make(map[domain.AccountID]struct{}),
		ACHLimitMinorUnits: 2_499_999, // $24,999.99
	}
}

// routingRule is a single routing decision step.
type routingRule interface {
	// match returns the rails and true when this rule applies, or false otherwise.
	match(ctx context.Context, p *domain.Payment, cfg RouterConfig) (domain.PaymentRails, bool)
}

// RuleBasedRouter evaluates routing rules in priority order and returns the
// first match. It implements PaymentRouter.
type RuleBasedRouter struct {
	cfg   RouterConfig
	rules []routingRule
}

// NewRuleBasedRouter constructs a RuleBasedRouter with the standard rule set.
func NewRuleBasedRouter(cfg RouterConfig) *RuleBasedRouter {
	return &RuleBasedRouter{
		cfg: cfg,
		rules: []routingRule{
			&internalRule{},
			&achRule{},
			&sepaRule{},
			&cryptoRule{},
			&defaultRule{},
		},
	}
}

// Route implements PaymentRouter.
func (r *RuleBasedRouter) Route(ctx context.Context, p *domain.Payment) (domain.PaymentRails, error) {
	for _, rule := range r.rules {
		if rails, matched := rule.match(ctx, p, r.cfg); matched {
			return rails, nil
		}
	}
	// defaultRule always matches, so this should never be reached.
	return domain.RailsSWIFT, fmt.Errorf("no routing rule matched for payment %s", p.ID)
}

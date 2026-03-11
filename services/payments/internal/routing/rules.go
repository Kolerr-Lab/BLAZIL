// Package routing implements payment rail selection logic.
package routing

import (
	"context"

	"github.com/blazil/services/payments/internal/domain"
)

// internalRule selects RailsInternal when both accounts belong to this Blazil instance.
type internalRule struct{}

func (r *internalRule) match(_ context.Context, p *domain.Payment, cfg RouterConfig) (domain.PaymentRails, bool) {
	_, debitLocal := cfg.LocalAccounts[p.DebitAccountID]
	_, creditLocal := cfg.LocalAccounts[p.CreditAccountID]
	if debitLocal && creditLocal {
		return domain.RailsInternal, true
	}
	return 0, false
}

// achRule selects RailsACH for USD payments below the same-day ACH limit.
type achRule struct{}

func (r *achRule) match(_ context.Context, p *domain.Payment, cfg RouterConfig) (domain.PaymentRails, bool) {
	if p.Amount.Currency.Code == "USD" && p.Amount.MinorUnits < cfg.ACHLimitMinorUnits {
		return domain.RailsACH, true
	}
	return 0, false
}

// sepaRule selects RailsSEPA for EUR payments.
type sepaRule struct{}

func (r *sepaRule) match(_ context.Context, p *domain.Payment, _ RouterConfig) (domain.PaymentRails, bool) {
	if p.Amount.Currency.Code == "EUR" {
		return domain.RailsSEPA, true
	}
	return 0, false
}

// cryptoRule selects RailsCrypto for BTC and ETH payments.
type cryptoRule struct{}

func (r *cryptoRule) match(_ context.Context, p *domain.Payment, _ RouterConfig) (domain.PaymentRails, bool) {
	code := p.Amount.Currency.Code
	if code == "BTC" || code == "ETH" {
		return domain.RailsCrypto, true
	}
	return 0, false
}

// defaultRule is the catch-all that always selects RailsSWIFT.
type defaultRule struct{}

func (r *defaultRule) match(_ context.Context, _ *domain.Payment, _ RouterConfig) (domain.PaymentRails, bool) {
	return domain.RailsSWIFT, true
}

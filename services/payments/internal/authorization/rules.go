// Package authorization implements the payment authorization engine.
package authorization

import (
	"context"

	"github.com/blazil/services/payments/internal/domain"
)

// zeroAmountRule rejects payments with a zero or negative amount.
type zeroAmountRule struct{}

func (r *zeroAmountRule) evaluate(_ context.Context, p *domain.Payment, _ AuthorizerConfig) (bool, bool, string) {
	if p.Amount.IsZero() || p.Amount.IsNegative() {
		return false, false, "payment amount must be positive"
	}
	return true, false, ""
}

// sameAccountRule rejects payments where debit and credit accounts are identical.
type sameAccountRule struct{}

func (r *sameAccountRule) evaluate(_ context.Context, p *domain.Payment, _ AuthorizerConfig) (bool, bool, string) {
	if p.DebitAccountID == p.CreditAccountID {
		return false, false, "debit and credit accounts must differ"
	}
	return true, false, ""
}

// currencyConsistencyRule rejects payments whose currency does not match the
// expected currency for the specified ledger.
// Crypto currencies (BTC, ETH) trigger a warning even when approved.
type currencyConsistencyRule struct{}

func (r *currencyConsistencyRule) evaluate(_ context.Context, p *domain.Payment, cfg AuthorizerConfig) (bool, bool, string) {
	expected, ok := cfg.LedgerCurrencies[p.LedgerID]
	if !ok {
		// Unknown ledger — allow through but warn.
		return true, true, ""
	}
	if p.Amount.Currency.Code != expected {
		return false, false, "currency mismatch for ledger"
	}
	// Warn on crypto currencies to flag elevated volatility risk.
	isCrypto := p.Amount.Currency.Code == "BTC" || p.Amount.Currency.Code == "ETH"
	return true, isCrypto, ""
}

// maxAmountRule rejects payments exceeding the configured single-payment limit.
// Payments above the warning threshold trigger a risk warning but are not rejected.
type maxAmountRule struct{}

func (r *maxAmountRule) evaluate(_ context.Context, p *domain.Payment, cfg AuthorizerConfig) (bool, bool, string) {
	if p.Amount.MinorUnits > cfg.MaxAmountMinorUnits {
		return false, false, "amount exceeds single-payment limit"
	}
	warned := cfg.WarnAmountMinorUnits > 0 && p.Amount.MinorUnits > cfg.WarnAmountMinorUnits
	return true, warned, ""
}

// metadataRule rejects payments missing the required "reference" metadata key.
type metadataRule struct{}

func (r *metadataRule) evaluate(_ context.Context, p *domain.Payment, _ AuthorizerConfig) (bool, bool, string) {
	if p.Metadata == nil {
		return false, false, "payment reference is required"
	}
	if _, ok := p.Metadata["reference"]; !ok {
		return false, false, "payment reference is required"
	}
	return true, false, ""
}

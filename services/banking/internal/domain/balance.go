// Package domain contains core banking domain types for the Blazil banking service.
package domain

import "time"

// Balance represents the canonical live balance for an account.
// It is the authoritative source of truth for an account's current funds;
// Account.BalanceMinorUnits stores only the opening balance.
type Balance struct {
	// AccountID identifies which account this balance belongs to.
	AccountID AccountID

	// MinorUnits is the current balance in the smallest currency unit (e.g. cents).
	MinorUnits int64

	// CurrencyCode is the ISO 4217 code.
	CurrencyCode string

	// UpdatedAt is the UTC time of the last mutation.
	UpdatedAt time.Time
}

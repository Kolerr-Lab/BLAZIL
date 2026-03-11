// Package domain contains core banking domain types for the Blazil banking service.
package domain

import "time"

// AccountID uniquely identifies a bank account.
type AccountID string

// AccountStatus represents the lifecycle state of an account.
type AccountStatus int

const (
	// AccountStatusActive means the account is open and operational.
	AccountStatusActive AccountStatus = iota
	// AccountStatusClosed means the account has been permanently closed.
	AccountStatusClosed
	// AccountStatusFrozen means the account is temporarily suspended.
	AccountStatusFrozen
)

// String returns a human-readable status name.
func (s AccountStatus) String() string {
	switch s {
	case AccountStatusActive:
		return "active"
	case AccountStatusClosed:
		return "closed"
	case AccountStatusFrozen:
		return "frozen"
	default:
		return "unknown"
	}
}

// AccountType classifies the account for interest and regulatory purposes.
type AccountType int

const (
	// AccountTypeChecking is a demand-deposit (current) account.
	AccountTypeChecking AccountType = iota
	// AccountTypeSavings earns interest on balances.
	AccountTypeSavings
	// AccountTypeLoan is a lending account with a negative amortising balance.
	AccountTypeLoan
)

// String returns a human-readable account type name.
func (t AccountType) String() string {
	switch t {
	case AccountTypeChecking:
		return "checking"
	case AccountTypeSavings:
		return "savings"
	case AccountTypeLoan:
		return "loan"
	default:
		return "unknown"
	}
}

// Account represents a single bank account.
type Account struct {
	// ID is the unique account identifier.
	ID AccountID

	// OwnerID is the external customer / entity identifier.
	OwnerID string

	// Type classifies the account.
	Type AccountType

	// CurrencyCode is the ISO 4217 code, e.g. "USD".
	CurrencyCode string

	// BalanceMinorUnits is the current balance in the smallest currency unit (e.g. cents).
	// For loan accounts this may be negative (outstanding principal).
	BalanceMinorUnits int64

	// Status is the current lifecycle state.
	Status AccountStatus

	// CreatedAt is the UTC timestamp of account creation.
	CreatedAt time.Time

	// UpdatedAt is the UTC timestamp of the last modification.
	UpdatedAt time.Time
}

// Package domain contains core banking domain types for the Blazil banking service.
package domain

import "time"

// TransactionID uniquely identifies a transaction.
type TransactionID string

// TransactionType classifies the direction and nature of a transaction.
type TransactionType int

const (
	// TransactionTypeDebit reduces the account balance.
	TransactionTypeDebit TransactionType = iota
	// TransactionTypeCredit increases the account balance.
	TransactionTypeCredit
	// TransactionTypeInterest is a credit applied by the interest engine.
	TransactionTypeInterest
	// TransactionTypeFee is a debit applied as a service charge.
	TransactionTypeFee
)

// String returns a human-readable transaction type name.
func (t TransactionType) String() string {
	switch t {
	case TransactionTypeDebit:
		return "debit"
	case TransactionTypeCredit:
		return "credit"
	case TransactionTypeInterest:
		return "interest"
	case TransactionTypeFee:
		return "fee"
	default:
		return "unknown"
	}
}

// Transaction represents a single ledger entry against an account.
type Transaction struct {
	// ID is the unique transaction identifier.
	ID TransactionID

	// AccountID is the account this transaction belongs to.
	AccountID AccountID

	// Type classifies the transaction.
	Type TransactionType

	// AmountMinorUnits is the absolute amount in the smallest currency unit.
	// Always non-negative; direction is determined by Type.
	AmountMinorUnits int64

	// CurrencyCode is the ISO 4217 code of the transaction currency.
	CurrencyCode string

	// BalanceAfterMinorUnits is the account balance immediately after this entry.
	BalanceAfterMinorUnits int64

	// Description is a human-readable memo supplied by the caller.
	Description string

	// Reference is an optional external reference number (e.g. payment ID).
	Reference string

	// Timestamp is the UTC time the transaction was recorded.
	Timestamp time.Time
}

// Package domain contains core payment domain types for the Blazil payments service.
package domain

import "time"

// PaymentID is a unique payment identifier (UUID v4 string).
type PaymentID string

// AccountID identifies a Blazil account, matching the Rust AccountId type.
type AccountID string

// LedgerID identifies a ledger in TigerBeetle. USD=1, EUR=2, GBP=3, etc.
type LedgerID uint32

// PaymentStatus represents the lifecycle state of a payment.
type PaymentStatus int

const (
	// StatusPending is the initial state before authorization.
	StatusPending PaymentStatus = iota
	// StatusAuthorized means the payment passed authorization checks.
	StatusAuthorized
	// StatusCleared means the payment is cleared but not yet settled (external rails).
	StatusCleared
	// StatusSettled means the payment is fully settled in the ledger.
	StatusSettled
	// StatusFailed means the payment was rejected at some point in the lifecycle.
	StatusFailed
	// StatusReversed means a previously settled payment was reversed.
	StatusReversed
)

// String returns a human-readable name for the payment status.
func (s PaymentStatus) String() string {
	switch s {
	case StatusPending:
		return "pending"
	case StatusAuthorized:
		return "authorized"
	case StatusCleared:
		return "cleared"
	case StatusSettled:
		return "settled"
	case StatusFailed:
		return "failed"
	case StatusReversed:
		return "reversed"
	default:
		return "unknown"
	}
}

// PaymentRails identifies which payment rail a payment is routed over.
type PaymentRails int

const (
	// RailsInternal routes the payment within the same Blazil instance via the Rust engine.
	RailsInternal PaymentRails = iota
	// RailsACH routes the payment over US ACH rails.
	RailsACH
	// RailsSEPA routes the payment over EU SEPA rails.
	RailsSEPA
	// RailsSWIFT routes the payment over international SWIFT rails.
	RailsSWIFT
	// RailsCrypto routes the payment over crypto rails (future).
	RailsCrypto
)

// String returns the canonical string name for a PaymentRails value.
func (r PaymentRails) String() string {
	switch r {
	case RailsInternal:
		return "internal"
	case RailsACH:
		return "ach"
	case RailsSEPA:
		return "sepa"
	case RailsSWIFT:
		return "swift"
	case RailsCrypto:
		return "crypto"
	default:
		return "unknown"
	}
}

// Payment represents a single payment in the Blazil system.
//
// All monetary values are stored as integer minor units to avoid floating-point
// precision errors on financial paths.
type Payment struct {
	// ID is the unique payment identifier assigned by the payments service.
	ID PaymentID

	// IdempotencyKey is a caller-provided deduplication key.
	// The same key always returns the same result.
	IdempotencyKey string

	// DebitAccountID is the account that funds are drawn from.
	DebitAccountID AccountID

	// CreditAccountID is the account that funds are credited to.
	CreditAccountID AccountID

	// Amount is the payment amount in minor units of the specified currency.
	Amount Money

	// LedgerID identifies the TigerBeetle ledger for this payment.
	LedgerID LedgerID

	// Rails is the payment rail selected by the routing engine.
	Rails PaymentRails

	// Status is the current lifecycle state.
	Status PaymentStatus

	// FailureReason contains a human-readable failure message when Status == StatusFailed.
	FailureReason string

	// CreatedAt is the UTC timestamp when the payment was first received.
	CreatedAt time.Time

	// UpdatedAt is the UTC timestamp of the last status change.
	UpdatedAt time.Time

	// Metadata is an arbitrary key-value map provided by the caller.
	// The "reference" key is required.
	Metadata map[string]string
}

// ProcessPaymentRequest is the input to the payment processor.
type ProcessPaymentRequest struct {
	// IdempotencyKey is the caller-provided deduplication key.
	IdempotencyKey string

	// DebitAccountID is the source account.
	DebitAccountID AccountID

	// CreditAccountID is the destination account.
	CreditAccountID AccountID

	// Amount is the payment amount.
	Amount Money

	// LedgerID identifies the target ledger.
	LedgerID LedgerID

	// Metadata is optional caller-provided key-value data.
	Metadata map[string]string
}

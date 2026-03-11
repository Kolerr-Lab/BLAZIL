// Package engine provides a local interface for the Blazil Rust engine used by
// the crypto service. No real network connections are made in tests.
package engine

import (
	"context"
	"fmt"
)

// EngineClient abstracts the Rust engine's debit/credit operations.
// Implementations must be safe for concurrent use.
type EngineClient interface {
	// Debit subtracts amount from the given account.
	Debit(ctx context.Context, accountID string, amount int64) error

	// Credit adds amount to the given account.
	Credit(ctx context.Context, accountID string, amount int64) error
}

// MockEngineClient is a configurable test double for EngineClient.
type MockEngineClient struct {
	DebitErr  error
	CreditErr error
	// Calls records "debit:{accountID}:{amount}" and "credit:{accountID}:{amount}" entries.
	Calls []string
}

// Debit records the call and returns DebitErr.
func (m *MockEngineClient) Debit(_ context.Context, accountID string, amount int64) error {
	m.Calls = append(m.Calls, fmt.Sprintf("debit:%s:%d", accountID, amount))
	return m.DebitErr
}

// Credit records the call and returns CreditErr.
func (m *MockEngineClient) Credit(_ context.Context, accountID string, amount int64) error {
	m.Calls = append(m.Calls, fmt.Sprintf("credit:%s:%d", accountID, amount))
	return m.CreditErr
}

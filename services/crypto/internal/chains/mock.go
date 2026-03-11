// Package chains defines the abstraction layer for blockchain interactions.
package chains

import (
	"context"
	"errors"
	"fmt"

	"github.com/blazil/crypto/internal/domain"
)

// feeTable maps ChainID to the static fee estimate in minor units.
var feeTable = map[domain.ChainID]int64{
	domain.ChainBitcoin:  10000,
	domain.ChainEthereum: 21000,
	domain.ChainPolygon:  1000,
	domain.ChainSolana:   1000,
	domain.ChainTron:     1000,
}

// MockChainAdapter is a deterministic, in-process ChainAdapter used in tests.
// It never makes real network calls.
type MockChainAdapter struct {
	chain domain.Chain
}

// NewMockChainAdapter constructs a MockChainAdapter for the given chain.
func NewMockChainAdapter(chain domain.Chain) *MockChainAdapter {
	return &MockChainAdapter{chain: chain}
}

// ChainID implements ChainAdapter.
func (m *MockChainAdapter) ChainID() domain.ChainID { return m.chain.ID }

// GenerateAddress implements ChainAdapter.
// Address format: "mock_{symbol}_{first 8 chars of ownerID}".
func (m *MockChainAdapter) GenerateAddress(_ context.Context, ownerID string) (string, error) {
	prefix := ownerID
	if len(prefix) > 8 {
		prefix = prefix[:8]
	}
	return fmt.Sprintf("mock_%s_%s", m.chain.Symbol, prefix), nil
}

// EstimateFee implements ChainAdapter.
// Returns the static fee from the fee table; ignores amount.
func (m *MockChainAdapter) EstimateFee(_ context.Context, _ int64) (int64, error) {
	fee, ok := feeTable[m.chain.ID]
	if !ok {
		return 0, domain.ErrChainNotSupported
	}
	return fee, nil
}

// BroadcastTx implements ChainAdapter.
// Returns "0xmock_{first 16 chars of withdrawal ID}".
// Returns an error if AmountMinorUnits == 1 (sentinel for forced failure in tests).
func (m *MockChainAdapter) BroadcastTx(_ context.Context, w *domain.Withdrawal) (string, error) {
	if w.AmountMinorUnits == 1 {
		return "", errors.New("mock broadcast failure")
	}
	txID := w.ID
	if len(txID) > 16 {
		txID = txID[:16]
	}
	return fmt.Sprintf("0xmock_%s", txID), nil
}

// GetConfirmations implements ChainAdapter.
// Always returns RequiredConfirmations+1 so deposits and withdrawals appear confirmed.
func (m *MockChainAdapter) GetConfirmations(_ context.Context, _ string) (int, error) {
	return m.chain.RequiredConfirmations + 1, nil
}

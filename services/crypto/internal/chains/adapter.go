// Package chains defines the abstraction layer for blockchain interactions.
package chains

import (
	"context"

	"github.com/blazil/crypto/internal/domain"
)

// ChainAdapter is the interface that each supported blockchain must implement.
// All implementations must be safe for concurrent use.
type ChainAdapter interface {
	// ChainID returns the chain this adapter handles.
	ChainID() domain.ChainID

	// GenerateAddress derives a deterministic deposit address for an owner.
	GenerateAddress(ctx context.Context, ownerID string) (string, error)

	// EstimateFee returns the fee in minor units for a transaction of the
	// given amount. No network call is required; a static estimate is fine.
	EstimateFee(ctx context.Context, amountMinorUnits int64) (int64, error)

	// BroadcastTx submits a withdrawal to the chain and returns the tx hash.
	BroadcastTx(ctx context.Context, w *domain.Withdrawal) (string, error)

	// GetConfirmations returns the current confirmation count for a tx hash.
	GetConfirmations(ctx context.Context, txHash string) (int, error)
}

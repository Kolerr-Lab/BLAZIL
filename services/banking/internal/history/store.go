// Package history implements transaction history storage for the Blazil banking service.
package history

import (
	"context"

	"github.com/blazil/banking/internal/domain"
)

// ListOptions controls pagination and filtering for transaction queries.
type ListOptions struct {
	// Limit is the maximum number of transactions to return (0 = no limit).
	Limit int
	// Offset is the number of transactions to skip (for page-based pagination).
	Offset int
}

// TransactionStore persists and retrieves Transaction records.
// All implementations must be safe for concurrent use.
type TransactionStore interface {
	// Append records a new transaction.
	Append(ctx context.Context, tx *domain.Transaction) error

	// GetByID returns the single transaction with the given ID.
	GetByID(ctx context.Context, id domain.TransactionID) (*domain.Transaction, error)

	// ListByAccount returns transactions for the given account, ordered by
	// Timestamp descending (newest first). opts controls pagination.
	ListByAccount(ctx context.Context, accountID domain.AccountID, opts ListOptions) ([]*domain.Transaction, error)
}

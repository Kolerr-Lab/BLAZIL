// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

import "time"

// WithdrawalStatus represents the processing state of a withdrawal.
type WithdrawalStatus string

const (
	WithdrawalStatusPending   WithdrawalStatus = "pending"
	WithdrawalStatusBroadcast WithdrawalStatus = "broadcast"
	WithdrawalStatusConfirmed WithdrawalStatus = "confirmed"
	WithdrawalStatusFailed    WithdrawalStatus = "failed"
)

// Withdrawal records an outbound on-chain transfer from a customer wallet.
type Withdrawal struct {
	ID               string
	WalletID         string
	AccountID        string // engine account to debit
	ToAddress        string
	ChainID          ChainID
	AmountMinorUnits int64
	FeeMinorUnits    int64
	TxHash           string
	Status           WithdrawalStatus
	CreatedAt        time.Time
}

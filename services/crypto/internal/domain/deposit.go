// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

import "time"

// DepositStatus represents the processing state of a deposit.
type DepositStatus string

const (
	DepositStatusDetected  DepositStatus = "detected"
	DepositStatusConfirmed DepositStatus = "confirmed"
	DepositStatusProcessed DepositStatus = "processed"
	DepositStatusFailed    DepositStatus = "failed"
)

// Deposit records an inbound on-chain transfer to a customer wallet.
type Deposit struct {
	ID               string
	WalletID         string
	AccountID        string // engine account to credit
	TxHash           string
	ChainID          ChainID
	AmountMinorUnits int64
	Status           DepositStatus
	Confirmations    int
	CreatedAt        time.Time
	ProcessedAt      *time.Time
}

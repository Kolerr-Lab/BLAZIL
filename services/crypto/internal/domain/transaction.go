// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

import "time"

// TxStatus represents the status of an on-chain transaction.
type TxStatus string

const (
	TxStatusPending   TxStatus = "pending"
	TxStatusConfirmed TxStatus = "confirmed"
	TxStatusFailed    TxStatus = "failed"
)

// CryptoTransaction records the details of an observed on-chain transaction.
type CryptoTransaction struct {
	TxHash           string
	ChainID          ChainID
	FromAddress      string
	ToAddress        string
	AmountMinorUnits int64
	FeeMinorUnits    int64
	Status           TxStatus
	Confirmations    int
	CreatedAt        time.Time
}

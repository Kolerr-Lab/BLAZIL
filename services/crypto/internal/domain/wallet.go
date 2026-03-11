// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

// WalletType classifies the purpose of a wallet.
type WalletType string

const (
	WalletTypeDeposit  WalletType = "deposit"
	WalletTypeWithdraw WalletType = "withdrawal"
	WalletTypeInternal WalletType = "internal"
)

// WalletStatus represents the operational state of a wallet.
type WalletStatus string

const (
	WalletStatusActive   WalletStatus = "active"
	WalletStatusFrozen   WalletStatus = "frozen"
	WalletStatusArchived WalletStatus = "archived"
)

// Wallet holds a customer-facing crypto wallet.
type Wallet struct {
	ID      string
	OwnerID string
	ChainID ChainID
	Address string
	Type    WalletType
	Status  WalletStatus
}

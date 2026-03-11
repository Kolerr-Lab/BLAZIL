// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

import "errors"

// Sentinel errors used throughout the crypto service.
var (
	// Wallet errors
	ErrWalletNotFound = errors.New("wallet not found")
	ErrWalletFrozen   = errors.New("wallet is frozen")
	ErrWalletArchived = errors.New("wallet is archived")

	// Chain errors
	ErrChainNotSupported = errors.New("chain not supported")
	ErrChainNotFound     = errors.New("chain not found")

	// Deposit errors
	ErrDepositNotFound         = errors.New("deposit not found")
	ErrDepositAlreadyProcessed = errors.New("deposit already processed")
	ErrNotEnoughConfirmations  = errors.New("not enough confirmations")

	// Withdrawal errors
	ErrWithdrawalNotFound   = errors.New("withdrawal not found")
	ErrWithdrawalNotPending = errors.New("withdrawal is not pending")
	ErrAmountBelowFee       = errors.New("amount is below fee")

	// General errors
	ErrInsufficientAmount = errors.New("amount must be positive")
	ErrChainMismatch      = errors.New("wallets are on different chains")
)

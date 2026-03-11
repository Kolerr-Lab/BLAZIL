// Package domain contains core banking domain types for the Blazil banking service.
package domain

import "errors"

// Sentinel errors used throughout the banking service.
var (
	// ErrAccountNotFound is returned when an account ID has no matching record.
	ErrAccountNotFound = errors.New("account not found")

	// ErrAccountAlreadyExists is returned when creating an account with a duplicate ID.
	ErrAccountAlreadyExists = errors.New("account already exists")

	// ErrInsufficientFunds is returned when a debit would bring the balance below zero.
	ErrInsufficientFunds = errors.New("insufficient funds")

	// ErrAccountClosed is returned when an operation is attempted on a closed account.
	ErrAccountClosed = errors.New("account is closed")

	// ErrAccountFrozen is returned when an operation is attempted on a frozen account.
	ErrAccountFrozen = errors.New("account is frozen")

	// ErrAccountNotActive is returned when an account must be active but is not.
	ErrAccountNotActive = errors.New("account is not active")

	// ErrAccountHasBalance is returned when closing an account with a non-zero balance.
	ErrAccountHasBalance = errors.New("account has non-zero balance")

	// ErrTransactionNotFound is returned when a transaction ID has no matching record.
	ErrTransactionNotFound = errors.New("transaction not found")

	// ErrNegativeAmount is returned when a monetary amount is negative.
	ErrNegativeAmount = errors.New("amount must be non-negative")
)

// Package domain contains core payment domain types for the Blazil payments service.
package domain

import "errors"

// ErrCurrencyMismatch is returned when an operation is attempted on two Money
// values with different currencies.
var ErrCurrencyMismatch = errors.New("currency mismatch")

// ErrPaymentNotFound is returned when a payment lookup finds no result.
var ErrPaymentNotFound = errors.New("payment not found")

// ValidationError wraps a field-level validation failure.
type ValidationError struct {
	// Field is the name of the domain field that failed validation.
	Field string

	// Message is a human-readable description of the validation failure.
	Message string
}

// Error implements the error interface.
func (e *ValidationError) Error() string {
	return "validation error on field " + e.Field + ": " + e.Message
}

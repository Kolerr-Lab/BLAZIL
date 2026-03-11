// Package domain contains core trading domain types for the Blazil trading service.
package domain

import "errors"

// Sentinel errors used throughout the trading service.
var (
	// ErrOrderNotFound is returned when an order ID has no matching record.
	ErrOrderNotFound = errors.New("order not found")

	// ErrOrderAlreadyExists is returned when placing an order with a duplicate ID.
	ErrOrderAlreadyExists = errors.New("order already exists")

	// ErrOrderNotOpen is returned when cancelling an order that is not open.
	ErrOrderNotOpen = errors.New("order is not open")

	// ErrInstrumentNotFound is returned when an instrument symbol has no matching record.
	ErrInstrumentNotFound = errors.New("instrument not found")

	// ErrInstrumentAlreadyExists is returned when registering a duplicate instrument.
	ErrInstrumentAlreadyExists = errors.New("instrument already exists")

	// ErrPositionNotFound is returned when a position lookup has no matching record.
	ErrPositionNotFound = errors.New("position not found")

	// ErrInvalidQuantity is returned when a quantity is zero or negative.
	ErrInvalidQuantity = errors.New("quantity must be positive")

	// ErrInvalidPrice is returned when a limit price is zero or negative.
	ErrInvalidPrice = errors.New("price must be positive")

	// ErrUnknownSide is returned when an order side is not buy or sell.
	ErrUnknownSide = errors.New("unknown order side")
)

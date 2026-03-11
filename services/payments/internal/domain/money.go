// Package domain contains core payment domain types for the Blazil payments service.
package domain

import (
	"fmt"
	"strings"
)

// Currency represents an ISO 4217 currency (or crypto currency).
type Currency struct {
	// Code is the ISO 4217 alphabetic code, e.g. "USD", "EUR", "BTC".
	Code string

	// Numeric is the ISO 4217 numeric code. Crypto currencies use 0.
	Numeric uint16

	// Decimals is the number of minor-unit decimal places.
	// USD=2 (cents), JPY=0, BTC=8 (satoshis), ETH=18 (wei).
	Decimals uint8
}

// Pre-defined currency constants covering all currencies required by the spec.
var (
	USD = Currency{Code: "USD", Numeric: 840, Decimals: 2}
	EUR = Currency{Code: "EUR", Numeric: 978, Decimals: 2}
	GBP = Currency{Code: "GBP", Numeric: 826, Decimals: 2}
	JPY = Currency{Code: "JPY", Numeric: 392, Decimals: 0}
	VND = Currency{Code: "VND", Numeric: 704, Decimals: 0}
	BTC = Currency{Code: "BTC", Numeric: 0, Decimals: 8}
	ETH = Currency{Code: "ETH", Numeric: 0, Decimals: 18}
)

// CurrencyByCode returns the Currency for the given ISO code, or an error if unknown.
func CurrencyByCode(code string) (Currency, error) {
	switch strings.ToUpper(code) {
	case "USD":
		return USD, nil
	case "EUR":
		return EUR, nil
	case "GBP":
		return GBP, nil
	case "JPY":
		return JPY, nil
	case "VND":
		return VND, nil
	case "BTC":
		return BTC, nil
	case "ETH":
		return ETH, nil
	default:
		return Currency{}, fmt.Errorf("unsupported currency code: %q", code)
	}
}

// Money is an immutable monetary value stored as integer minor units.
//
// CRITICAL: float64 is never used in any monetary computation. All arithmetic
// is performed in int64 minor units to guarantee exact representation.
type Money struct {
	// MinorUnits holds the amount in the smallest unit of the currency.
	// Examples: $10.50 USD = 1050, ¥100 JPY = 100, 0.001 BTC = 100000.
	MinorUnits int64

	// Currency identifies the denomination and decimal precision.
	Currency Currency
}

// NewMoney constructs a Money value from minor units and a currency.
func NewMoney(minorUnits int64, currency Currency) Money {
	return Money{MinorUnits: minorUnits, Currency: currency}
}

// IsZero returns true when the amount is exactly zero.
func (m Money) IsZero() bool {
	return m.MinorUnits == 0
}

// IsNegative returns true when the amount is less than zero.
func (m Money) IsNegative() bool {
	return m.MinorUnits < 0
}

// Add returns the sum of m and other.
// Returns an error if the currencies do not match.
func (m Money) Add(other Money) (Money, error) {
	if m.Currency.Code != other.Currency.Code {
		return Money{}, fmt.Errorf("%w: cannot add %s to %s",
			ErrCurrencyMismatch, other.Currency.Code, m.Currency.Code)
	}
	return NewMoney(m.MinorUnits+other.MinorUnits, m.Currency), nil
}

// String formats the money as a human-readable decimal string, e.g. "10.50 USD".
//
// No float64 arithmetic is used: the decimal point is inserted via integer
// division and modulo operations on string representations.
func (m Money) String() string {
	d := int(m.Currency.Decimals)
	if d == 0 {
		return fmt.Sprintf("%d %s", m.MinorUnits, m.Currency.Code)
	}

	// Work with the absolute value, remember the sign.
	abs := m.MinorUnits
	sign := ""
	if abs < 0 {
		abs = -abs
		sign = "-"
	}

	// Build the raw digit string, zero-padded to at least d+1 digits.
	raw := fmt.Sprintf("%d", abs)
	for len(raw) <= d {
		raw = "0" + raw
	}

	// Insert decimal point d digits from the right.
	intPart := raw[:len(raw)-d]
	fracPart := raw[len(raw)-d:]

	return fmt.Sprintf("%s%s.%s %s", sign, intPart, fracPart, m.Currency.Code)
}

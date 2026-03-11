package domain

import "time"

// InstrumentID uniquely identifies a tradeable instrument.
type InstrumentID string

// Instrument represents a tradeable financial instrument (equity, FX pair, etc.).
type Instrument struct {
	ID           InstrumentID
	Symbol       string
	BaseCurrency string // ISO 4217, e.g. "USD" — price is quoted in this
	Description  string
	CreatedAt    time.Time
}

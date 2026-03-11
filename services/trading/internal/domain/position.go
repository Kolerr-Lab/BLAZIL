package domain

import "time"

// PositionID uniquely identifies a position record.
type PositionID string

// Position represents an owner's net holding in a single instrument.
// QuantityUnits > 0 means long; < 0 would mean short (not supported in v1).
// AverageCostMinorUnits is the volume-weighted average purchase price.
type Position struct {
	ID                    PositionID
	OwnerID               string
	InstrumentID          InstrumentID
	QuantityUnits         int64
	AverageCostMinorUnits int64
	UpdatedAt             time.Time
}

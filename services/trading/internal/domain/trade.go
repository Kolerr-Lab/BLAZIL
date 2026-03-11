package domain

import "time"

// TradeID uniquely identifies an executed trade.
type TradeID string

// Trade records a matched fill between a maker (resting) and taker (aggressive) order.
// The trade price is always the maker's limit price (price-time priority / FIFO).
// All monetary values are in minor units.
type Trade struct {
	ID           TradeID
	InstrumentID InstrumentID
	// MakerOrderID is the resting order that was matched against.
	MakerOrderID OrderID
	// TakerOrderID is the aggressive order that triggered the match.
	TakerOrderID OrderID
	// PriceMinorUnits is the execution price (= maker's limit price).
	PriceMinorUnits int64
	// QuantityUnits is the filled quantity in this trade.
	QuantityUnits int64
	ExecutedAt    time.Time
}

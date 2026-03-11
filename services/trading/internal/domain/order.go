package domain

import "time"

// OrderID uniquely identifies an order.
type OrderID string

// Side is the direction of an order (buy or sell).
type Side int

const (
	SideBuy  Side = iota + 1 // 1
	SideSell                 // 2
)

// String returns the human-readable side name.
func (s Side) String() string {
	switch s {
	case SideBuy:
		return "buy"
	case SideSell:
		return "sell"
	default:
		return "unknown"
	}
}

// OrderStatus represents the lifecycle state of an order.
type OrderStatus int

const (
	OrderStatusOpen      OrderStatus = iota + 1
	OrderStatusPartial                // partially filled
	OrderStatusFilled                 // fully filled
	OrderStatusCancelled              // cancelled by user
)

// String returns the human-readable order status.
func (s OrderStatus) String() string {
	switch s {
	case OrderStatusOpen:
		return "open"
	case OrderStatusPartial:
		return "partial"
	case OrderStatusFilled:
		return "filled"
	case OrderStatusCancelled:
		return "cancelled"
	default:
		return "unknown"
	}
}

// Order represents a resting or newly-placed limit order.
// All monetary values are in minor units (e.g. cents for USD).
type Order struct {
	ID           OrderID
	InstrumentID InstrumentID
	OwnerID      string
	Side         Side
	// LimitPriceMinorUnits is the maximum (buy) or minimum (sell) price.
	LimitPriceMinorUnits int64
	// QuantityUnits is the total ordered quantity (e.g. shares).
	QuantityUnits int64
	// FilledUnits is the quantity already matched.
	FilledUnits int64
	Status      OrderStatus
	PlacedAt    time.Time
	UpdatedAt   time.Time
}

// RemainingUnits returns the unfilled quantity.
func (o *Order) RemainingUnits() int64 {
	return o.QuantityUnits - o.FilledUnits
}

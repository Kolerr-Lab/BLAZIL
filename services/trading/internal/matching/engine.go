// Package matching implements the FIFO price-time priority matching engine.
package matching

import (
	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/orderbook"
)

// MatchResult contains all fills produced by matching one taker order.
type MatchResult struct {
	// Trades holds each individual fill in execution order.
	Trades []*domain.Trade
	// FilledMakers holds maker orders whose status was mutated during matching.
	FilledMakers []*domain.Order
	// Taker is the incoming order after matching (status updated).
	Taker *domain.Order
}

// Engine is the interface for a matching engine operating on a single order book.
type Engine interface {
	// Match attempts to fill taker against resting orders in book.
	// Resting orders and the taker are mutated in place.
	// The book must already contain all resting orders.
	// Returns MatchResult with all fills and updated orders.
	Match(book *orderbook.Book, taker *domain.Order, newTradeID func() domain.TradeID) MatchResult
}

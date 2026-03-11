package matching

import (
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/orderbook"
)

// FIFOEngine implements strict price-time (FIFO) priority matching.
//
// Matching rules:
//   - A buy (taker) matches against asks at or below its limit price.
//   - A sell (taker) matches against bids at or above its limit price.
//   - Within a price level, orders are filled in arrival (FIFO) order.
//   - Trade price = maker (resting) order's limit price.
//   - Taker order's filled quantity is incremented for each fill.
//
// No panics; all error conditions are handled through MatchResult.
type FIFOEngine struct{}

// NewFIFOEngine constructs a FIFOEngine.
func NewFIFOEngine() *FIFOEngine { return &FIFOEngine{} }

// Match implements Engine.
func (e *FIFOEngine) Match(book *orderbook.Book, taker *domain.Order, newTradeID func() domain.TradeID) MatchResult {
	result := MatchResult{Taker: taker}

	for taker.RemainingUnits() > 0 {
		level, makerOrder := e.bestMatchingLevel(book, taker)
		if makerOrder == nil {
			break // no more matching resting orders
		}

		fillQty := min64(taker.RemainingUnits(), makerOrder.RemainingUnits())
		now := time.Now().UTC()

		trade := &domain.Trade{
			ID:              newTradeID(),
			InstrumentID:    taker.InstrumentID,
			MakerOrderID:    makerOrder.ID,
			TakerOrderID:    taker.ID,
			PriceMinorUnits: makerOrder.LimitPriceMinorUnits,
			QuantityUnits:   fillQty,
			ExecutedAt:      now,
		}
		result.Trades = append(result.Trades, trade)

		// Update maker.
		makerOrder.FilledUnits += fillQty
		makerOrder.UpdatedAt = now
		if makerOrder.FilledUnits >= makerOrder.QuantityUnits {
			// Cancel from book BEFORE setting status to Filled (Cancel checks for Open/Partial).
			_ = book.Cancel(makerOrder.ID)
			makerOrder.Status = domain.OrderStatusFilled
		} else {
			makerOrder.Status = domain.OrderStatusPartial
		}
		result.FilledMakers = append(result.FilledMakers, makerOrder)

		// Update taker.
		taker.FilledUnits += fillQty
		taker.UpdatedAt = now
		if taker.FilledUnits >= taker.QuantityUnits {
			taker.Status = domain.OrderStatusFilled
		} else {
			taker.Status = domain.OrderStatusPartial
		}

		_ = level // level reference used only to iterate; book.Cancel handles removal
	}

	return result
}

// bestMatchingLevel finds the first resting order that crosses with taker.
// Returns the level snapshot (not used by caller but confirms the iteration),
// and the maker order if a match exists, or nil.
func (e *FIFOEngine) bestMatchingLevel(book *orderbook.Book, taker *domain.Order) (interface{}, *domain.Order) {
	if taker.Side == domain.SideBuy {
		// Buy taker matches against best (lowest) ask.
		bestAsk, ok := book.BestAsk()
		if !ok || bestAsk > taker.LimitPriceMinorUnits {
			return nil, nil
		}
		// Get the first FIFO order at that level via AskLevels snapshot
		// and look it up through the public Cancel/Add surface — but we need
		// the actual order pointer.  AskLevels only returns snapshots.
		// We expose the internal best-level order via a helper instead.
		return nil, e.firstOrderAtAskLevel(book, bestAsk)
	}
	// Sell taker matches against best (highest) bid.
	bestBid, ok := book.BestBid()
	if !ok || bestBid < taker.LimitPriceMinorUnits {
		return nil, nil
	}
	return nil, e.firstOrderAtBidLevel(book, bestBid)
}

// firstOrderAtAskLevel returns the oldest (FIFO) order at the given ask price.
func (e *FIFOEngine) firstOrderAtAskLevel(book *orderbook.Book, price int64) *domain.Order {
	for _, snap := range book.AskLevels() {
		if snap.PriceMinorUnits == price {
			// We need the actual orders — use the helper exposed by Book.
			orders := book.OrdersAtAskPrice(price)
			if len(orders) > 0 {
				return orders[0]
			}
		}
	}
	return nil
}

// firstOrderAtBidLevel returns the oldest (FIFO) order at the given bid price.
func (e *FIFOEngine) firstOrderAtBidLevel(book *orderbook.Book, price int64) *domain.Order {
	for _, snap := range book.BidLevels() {
		if snap.PriceMinorUnits == price {
			orders := book.OrdersAtBidPrice(price)
			if len(orders) > 0 {
				return orders[0]
			}
		}
	}
	return nil
}

func min64(a, b int64) int64 {
	if a < b {
		return a
	}
	return b
}

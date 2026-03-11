// Package orderbook provides a thread-safe price-level order book for a single instrument.
package orderbook

import (
	"sort"
	"sync"

	"github.com/blazil/trading/internal/domain"
)

// PriceLevel holds all resting orders at a single price point,
// maintained in FIFO (arrival) order within the level.
type PriceLevel struct {
	PriceMinorUnits int64
	orders          []*domain.Order // FIFO
}

// Orders returns a read-only view of orders at this price level.
func (pl *PriceLevel) Orders() []*domain.Order {
	cp := make([]*domain.Order, len(pl.orders))
	copy(cp, pl.orders)
	return cp
}

// totalRemaining returns the sum of remaining units across all orders in the level.
func (pl *PriceLevel) totalRemaining() int64 {
	var total int64
	for _, o := range pl.orders {
		total += o.RemainingUnits()
	}
	return total
}

// Book is a thread-safe limit order book for a single instrument.
//
// Bid levels are sorted descending (best bid = highest price first).
// Ask levels are sorted ascending (best ask = lowest price first).
type Book struct {
	mu           sync.RWMutex
	instrumentID domain.InstrumentID
	bids         []*PriceLevel // sorted descending by price
	asks         []*PriceLevel // sorted ascending by price
	orderIndex   map[domain.OrderID]*domain.Order
}

// New constructs an empty Book for the given instrument.
func New(id domain.InstrumentID) *Book {
	return &Book{
		instrumentID: id,
		orderIndex:   make(map[domain.OrderID]*domain.Order),
	}
}

// InstrumentID returns the instrument this book belongs to.
func (b *Book) InstrumentID() domain.InstrumentID {
	return b.instrumentID
}

// Add inserts a resting order into the book.
// The order must already have been validated (positive qty, positive price, correct side).
// Returns ErrOrderAlreadyExists if the order ID is already present.
func (b *Book) Add(o *domain.Order) error {
	b.mu.Lock()
	defer b.mu.Unlock()

	if _, exists := b.orderIndex[o.ID]; exists {
		return domain.ErrOrderAlreadyExists
	}

	b.orderIndex[o.ID] = o
	if o.Side == domain.SideBuy {
		b.bids = addToLevels(b.bids, o, true /* descending */)
	} else {
		b.asks = addToLevels(b.asks, o, false /* ascending */)
	}
	return nil
}

// Cancel removes an open order from the book.
// Returns ErrOrderNotFound if unknown, ErrOrderNotOpen if already filled/cancelled.
func (b *Book) Cancel(id domain.OrderID) error {
	b.mu.Lock()
	defer b.mu.Unlock()

	o, exists := b.orderIndex[id]
	if !exists {
		return domain.ErrOrderNotFound
	}
	if o.Status != domain.OrderStatusOpen && o.Status != domain.OrderStatusPartial {
		return domain.ErrOrderNotOpen
	}

	if o.Side == domain.SideBuy {
		b.bids = removeFromLevels(b.bids, o)
	} else {
		b.asks = removeFromLevels(b.asks, o)
	}
	delete(b.orderIndex, id)
	return nil
}

// BestBid returns the highest resting bid price, or (0, false) if no bids.
func (b *Book) BestBid() (int64, bool) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	if len(b.bids) == 0 {
		return 0, false
	}
	return b.bids[0].PriceMinorUnits, true
}

// BestAsk returns the lowest resting ask price, or (0, false) if no asks.
func (b *Book) BestAsk() (int64, bool) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	if len(b.asks) == 0 {
		return 0, false
	}
	return b.asks[0].PriceMinorUnits, true
}

// BidLevels returns a snapshot of all bid price levels (descending).
func (b *Book) BidLevels() []LevelSnapshot {
	b.mu.RLock()
	defer b.mu.RUnlock()
	return snapshotLevels(b.bids)
}

// AskLevels returns a snapshot of all ask price levels (ascending).
func (b *Book) AskLevels() []LevelSnapshot {
	b.mu.RLock()
	defer b.mu.RUnlock()
	return snapshotLevels(b.asks)
}

// OrdersAtBidPrice returns the live orders at the given bid price in FIFO order.
// Returns nil if no level exists at that price.
func (b *Book) OrdersAtBidPrice(priceMinorUnits int64) []*domain.Order {
	b.mu.RLock()
	defer b.mu.RUnlock()
	for _, pl := range b.bids {
		if pl.PriceMinorUnits == priceMinorUnits {
			return pl.Orders()
		}
	}
	return nil
}

// OrdersAtAskPrice returns the live orders at the given ask price in FIFO order.
// Returns nil if no level exists at that price.
func (b *Book) OrdersAtAskPrice(priceMinorUnits int64) []*domain.Order {
	b.mu.RLock()
	defer b.mu.RUnlock()
	for _, pl := range b.asks {
		if pl.PriceMinorUnits == priceMinorUnits {
			return pl.Orders()
		}
	}
	return nil
}

// LevelSnapshot is an immutable view of a price level at a point in time.
type LevelSnapshot struct {
	PriceMinorUnits int64
	TotalUnits      int64
	OrderCount      int
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers (no locks required; callers hold the lock)
// ─────────────────────────────────────────────────────────────────────────────

// addToLevels inserts order o into the sorted levels slice, creating a new
// PriceLevel if necessary. descending=true for bids (highest price first).
func addToLevels(levels []*PriceLevel, o *domain.Order, descending bool) []*PriceLevel {
	price := o.LimitPriceMinorUnits
	idx := sort.Search(len(levels), func(i int) bool {
		if descending {
			return levels[i].PriceMinorUnits <= price
		}
		return levels[i].PriceMinorUnits >= price
	})
	if idx < len(levels) && levels[idx].PriceMinorUnits == price {
		// Existing level — append in arrival (FIFO) order.
		levels[idx].orders = append(levels[idx].orders, o)
		return levels
	}
	// New level — insert at idx.
	nl := &PriceLevel{PriceMinorUnits: price, orders: []*domain.Order{o}}
	levels = append(levels, nil)
	copy(levels[idx+1:], levels[idx:])
	levels[idx] = nl
	return levels
}

// removeFromLevels removes order o from levels.
func removeFromLevels(levels []*PriceLevel, o *domain.Order) []*PriceLevel {
	for li, pl := range levels {
		if pl.PriceMinorUnits != o.LimitPriceMinorUnits {
			continue
		}
		for oi, ord := range pl.orders {
			if ord.ID == o.ID {
				pl.orders = append(pl.orders[:oi], pl.orders[oi+1:]...)
				if len(pl.orders) == 0 {
					return append(levels[:li], levels[li+1:]...)
				}
				return levels
			}
		}
	}
	return levels
}

// filterNonEmpty removes price levels whose orders slice is empty.
func filterNonEmpty(levels []*PriceLevel) []*PriceLevel {
	out := levels[:0]
	for _, pl := range levels {
		if len(pl.orders) > 0 {
			out = append(out, pl)
		}
	}
	return out
}

func snapshotLevels(levels []*PriceLevel) []LevelSnapshot {
	out := make([]LevelSnapshot, len(levels))
	for i, pl := range levels {
		out[i] = LevelSnapshot{
			PriceMinorUnits: pl.PriceMinorUnits,
			TotalUnits:      pl.totalRemaining(),
			OrderCount:      len(pl.orders),
		}
	}
	return out
}

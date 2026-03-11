// Package orders implements order lifecycle management for the trading service.
package orders

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orderbook"
)

// PlaceOrderRequest is the input for OrderService.PlaceOrder.
type PlaceOrderRequest struct {
	ID                   domain.OrderID
	InstrumentID         domain.InstrumentID
	OwnerID              string
	Side                 domain.Side
	LimitPriceMinorUnits int64
	QuantityUnits        int64
}

// OrderService manages limit order placement, cancellation, and querying.
type OrderService interface {
	PlaceOrder(ctx context.Context, req PlaceOrderRequest) (*domain.Order, []domain.Trade, error)
	CancelOrder(ctx context.Context, id domain.OrderID) error
	GetOrder(ctx context.Context, id domain.OrderID) (*domain.Order, error)
	ListByOwner(ctx context.Context, ownerID string) ([]*domain.Order, error)
}

// InMemoryOrderService is a thread-safe in-memory OrderService.
// It owns one order book per instrument and uses the FIFO matching engine.
type InMemoryOrderService struct {
	mu      sync.RWMutex
	orders  map[domain.OrderID]*domain.Order
	books   map[domain.InstrumentID]*orderbook.Book
	engine  matching.Engine
	tradeID int64 // monotonic counter for trade IDs
}

// NewInMemoryOrderService constructs an InMemoryOrderService.
func NewInMemoryOrderService(engine matching.Engine) *InMemoryOrderService {
	return &InMemoryOrderService{
		orders: make(map[domain.OrderID]*domain.Order),
		books:  make(map[domain.InstrumentID]*orderbook.Book),
		engine: engine,
	}
}

// PlaceOrder implements OrderService.
func (s *InMemoryOrderService) PlaceOrder(_ context.Context, req PlaceOrderRequest) (*domain.Order, []domain.Trade, error) {
	if req.QuantityUnits <= 0 {
		return nil, nil, domain.ErrInvalidQuantity
	}
	if req.LimitPriceMinorUnits <= 0 {
		return nil, nil, domain.ErrInvalidPrice
	}
	if req.Side != domain.SideBuy && req.Side != domain.SideSell {
		return nil, nil, domain.ErrUnknownSide
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	if _, exists := s.orders[req.ID]; exists {
		return nil, nil, domain.ErrOrderAlreadyExists
	}

	now := time.Now().UTC()
	order := &domain.Order{
		ID:                   req.ID,
		InstrumentID:         req.InstrumentID,
		OwnerID:              req.OwnerID,
		Side:                 req.Side,
		LimitPriceMinorUnits: req.LimitPriceMinorUnits,
		QuantityUnits:        req.QuantityUnits,
		Status:               domain.OrderStatusOpen,
		PlacedAt:             now,
		UpdatedAt:            now,
	}
	s.orders[order.ID] = order

	book := s.bookForInstrument(req.InstrumentID)

	// Match the incoming order against the opposite side.
	result := s.engine.Match(book, order, s.nextTradeID)

	// If not fully filled, add to the book as a resting order.
	if order.Status != domain.OrderStatusFilled {
		if err := book.Add(order); err != nil {
			// Should not happen (we checked for duplicate above), but guard anyway.
			return nil, nil, fmt.Errorf("add to book: %w", err)
		}
	}

	var trades []domain.Trade
	for _, t := range result.Trades {
		trades = append(trades, *t)
	}
	return order, trades, nil
}

// CancelOrder implements OrderService.
func (s *InMemoryOrderService) CancelOrder(_ context.Context, id domain.OrderID) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	o, exists := s.orders[id]
	if !exists {
		return domain.ErrOrderNotFound
	}
	if o.Status != domain.OrderStatusOpen && o.Status != domain.OrderStatusPartial {
		return domain.ErrOrderNotOpen
	}

	book := s.bookForInstrument(o.InstrumentID)
	if err := book.Cancel(id); err != nil {
		return fmt.Errorf("cancel from book: %w", err)
	}
	o.Status = domain.OrderStatusCancelled
	o.UpdatedAt = time.Now().UTC()
	return nil
}

// GetOrder implements OrderService.
func (s *InMemoryOrderService) GetOrder(_ context.Context, id domain.OrderID) (*domain.Order, error) {
	s.mu.RLock()
	o, ok := s.orders[id]
	s.mu.RUnlock()
	if !ok {
		return nil, domain.ErrOrderNotFound
	}
	return o, nil
}

// ListByOwner implements OrderService.
func (s *InMemoryOrderService) ListByOwner(_ context.Context, ownerID string) ([]*domain.Order, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var result []*domain.Order
	for _, o := range s.orders {
		if o.OwnerID == ownerID {
			result = append(result, o)
		}
	}
	return result, nil
}

// bookForInstrument returns the order book for the given instrument,
// creating one if it does not exist. Must be called with s.mu held.
func (s *InMemoryOrderService) bookForInstrument(id domain.InstrumentID) *orderbook.Book {
	if b, ok := s.books[id]; ok {
		return b
	}
	b := orderbook.New(id)
	s.books[id] = b
	return b
}

func (s *InMemoryOrderService) nextTradeID() domain.TradeID {
	s.tradeID++
	return domain.TradeID(fmt.Sprintf("trade-%d", s.tradeID))
}

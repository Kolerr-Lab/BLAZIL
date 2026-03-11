// Package settlement processes executed trades and updates positions.
package settlement

import (
	"context"
	"fmt"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/orders"
	"github.com/blazil/trading/internal/positions"
)

// Settler processes trades and applies them to the position book.
type Settler interface {
	// Settle takes a list of trades resulting from a single order placement,
	// resolves buyer/seller owner IDs from the order store, and updates positions.
	Settle(ctx context.Context, trades []domain.Trade) error
}

// EngineSettler is the standard implementation of Settler.
type EngineSettler struct {
	orders    orders.OrderService
	positions positions.PositionService
}

// NewEngineSettler constructs an EngineSettler.
func NewEngineSettler(orderSvc orders.OrderService, posSvc positions.PositionService) *EngineSettler {
	return &EngineSettler{orders: orderSvc, positions: posSvc}
}

// Settle implements Settler.
func (s *EngineSettler) Settle(ctx context.Context, trades []domain.Trade) error {
	for _, trade := range trades {
		makerOrder, err := s.orders.GetOrder(ctx, trade.MakerOrderID)
		if err != nil {
			return fmt.Errorf("settle: get maker order %s: %w", trade.MakerOrderID, err)
		}
		takerOrder, err := s.orders.GetOrder(ctx, trade.TakerOrderID)
		if err != nil {
			return fmt.Errorf("settle: get taker order %s: %w", trade.TakerOrderID, err)
		}

		// Determine buyer and seller owner IDs.
		var buyerOwnerID, sellerOwnerID string
		if takerOrder.Side == domain.SideBuy {
			buyerOwnerID = takerOrder.OwnerID
			sellerOwnerID = makerOrder.OwnerID
		} else {
			buyerOwnerID = makerOrder.OwnerID
			sellerOwnerID = takerOrder.OwnerID
		}

		if err := s.positions.ApplyTrade(ctx, trade, buyerOwnerID, sellerOwnerID); err != nil {
			return fmt.Errorf("settle: apply trade %s: %w", trade.ID, err)
		}
	}
	return nil
}

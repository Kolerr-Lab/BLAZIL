package settlement_test

import (
	"context"
	"testing"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orders"
	"github.com/blazil/trading/internal/positions"
	"github.com/blazil/trading/internal/settlement"
)

func setup() (*orders.InMemoryOrderService, *positions.InMemoryPositionService, *settlement.EngineSettler) {
	orderSvc := orders.NewInMemoryOrderService(matching.NewFIFOEngine())
	posSvc := positions.NewInMemoryPositionService()
	settler := settlement.NewEngineSettler(orderSvc, posSvc)
	return orderSvc, posSvc, settler
}

func TestSettle_UpdatesPositions(t *testing.T) {
	orderSvc, posSvc, settler := setup()
	ctx := context.Background()

	// Place a sell (resting maker).
	_, _, _ = orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "a1", InstrumentID: "AAPL", OwnerID: "seller",
		Side: domain.SideSell, LimitPriceMinorUnits: 100, QuantityUnits: 10,
	})
	// Place a buy (taker) that matches.
	_, trades, _ := orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "b1", InstrumentID: "AAPL", OwnerID: "buyer",
		Side: domain.SideBuy, LimitPriceMinorUnits: 100, QuantityUnits: 10,
	})
	if len(trades) == 0 {
		t.Fatal("expected trades from matching")
	}

	if err := settler.Settle(ctx, trades); err != nil {
		t.Fatalf("Settle: %v", err)
	}

	buyerPos, err := posSvc.GetPosition(ctx, "buyer", "AAPL")
	if err != nil {
		t.Fatalf("GetPosition buyer: %v", err)
	}
	if buyerPos.QuantityUnits != 10 {
		t.Errorf("buyer qty: want 10, got %d", buyerPos.QuantityUnits)
	}

	sellerPos, err := posSvc.GetPosition(ctx, "seller", "AAPL")
	if err != nil {
		t.Fatalf("GetPosition seller: %v", err)
	}
	if sellerPos.QuantityUnits != -10 {
		t.Errorf("seller qty: want -10, got %d", sellerPos.QuantityUnits)
	}
}

func TestSettle_TradePrice(t *testing.T) {
	orderSvc, posSvc, settler := setup()
	ctx := context.Background()

	_, _, _ = orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "a1", InstrumentID: "GOOG", OwnerID: "seller",
		Side: domain.SideSell, LimitPriceMinorUnits: 200, QuantityUnits: 5,
	})
	_, trades, _ := orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "b1", InstrumentID: "GOOG", OwnerID: "buyer",
		Side: domain.SideBuy, LimitPriceMinorUnits: 210, QuantityUnits: 5,
	})
	_ = settler.Settle(ctx, trades)

	pos, _ := posSvc.GetPosition(ctx, "buyer", "GOOG")
	// Trade price must be maker price = 200.
	if pos.AverageCostMinorUnits != 200 {
		t.Errorf("avg cost: want 200 (maker price), got %d", pos.AverageCostMinorUnits)
	}
}

func TestSettle_NoTrades_NoOp(t *testing.T) {
	_, _, settler := setup()
	if err := settler.Settle(context.Background(), nil); err != nil {
		t.Errorf("Settle with no trades: %v", err)
	}
}

func TestSettle_InfersBuyerFromTakerSide(t *testing.T) {
	// Verify buyer/seller inference: taker=buy, maker=sell.
	orderSvc, posSvc, settler := setup()
	ctx := context.Background()

	_, _, _ = orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "s1", InstrumentID: "TSLA", OwnerID: "alice",
		Side: domain.SideSell, LimitPriceMinorUnits: 300, QuantityUnits: 3,
	})
	_, trades, _ := orderSvc.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID: "b1", InstrumentID: "TSLA", OwnerID: "bob",
		Side: domain.SideBuy, LimitPriceMinorUnits: 300, QuantityUnits: 3,
	})
	_ = settler.Settle(ctx, trades)

	bobPos, _ := posSvc.GetPosition(ctx, "bob", "TSLA")
	if bobPos.QuantityUnits != 3 {
		t.Errorf("bob qty: want 3, got %d", bobPos.QuantityUnits)
	}
	// Use time package to ensure we're in the right timestamp range.
	if bobPos.UpdatedAt.IsZero() {
		t.Error("UpdatedAt should not be zero")
	}
	_ = time.Now() // reference
}

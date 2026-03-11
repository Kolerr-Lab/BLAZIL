package positions_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/positions"
)

func makeTrade(qty, price int64) domain.Trade {
	return domain.Trade{
		ID:              "trade-1",
		InstrumentID:    "AAPL",
		MakerOrderID:    "maker-1",
		TakerOrderID:    "taker-1",
		PriceMinorUnits: price,
		QuantityUnits:   qty,
		ExecutedAt:      time.Now().UTC(),
	}
}

func TestApplyTrade_BuyerPositionIncreases(t *testing.T) {
	svc := positions.NewInMemoryPositionService()
	ctx := context.Background()

	trade := makeTrade(10, 100)
	if err := svc.ApplyTrade(ctx, trade, "buyer", "seller"); err != nil {
		t.Fatalf("ApplyTrade: %v", err)
	}

	pos, err := svc.GetPosition(ctx, "buyer", "AAPL")
	if err != nil {
		t.Fatalf("GetPosition: %v", err)
	}
	if pos.QuantityUnits != 10 {
		t.Errorf("buyer qty: want 10, got %d", pos.QuantityUnits)
	}
	if pos.AverageCostMinorUnits != 100 {
		t.Errorf("buyer avg cost: want 100, got %d", pos.AverageCostMinorUnits)
	}
}

func TestApplyTrade_SellerPositionDecreases(t *testing.T) {
	svc := positions.NewInMemoryPositionService()
	ctx := context.Background()
	// Seller sells 10 to buyer.
	_ = svc.ApplyTrade(ctx, makeTrade(10, 100), "buyer", "seller")

	pos, err := svc.GetPosition(ctx, "seller", "AAPL")
	if err != nil {
		t.Fatalf("GetPosition: %v", err)
	}
	if pos.QuantityUnits != -10 {
		t.Errorf("seller qty: want -10, got %d", pos.QuantityUnits)
	}
}

func TestApplyTrade_WeightedAverageCost(t *testing.T) {
	svc := positions.NewInMemoryPositionService()
	ctx := context.Background()
	// Buy 10 @ 100 = avg 100.
	_ = svc.ApplyTrade(ctx, makeTrade(10, 100), "buyer", "s1")
	// Buy 10 @ 120 = new avg (100*10 + 120*10) / 20 = 110.
	trade2 := domain.Trade{
		ID: "t2", InstrumentID: "AAPL",
		PriceMinorUnits: 120, QuantityUnits: 10, ExecutedAt: time.Now().UTC(),
	}
	_ = svc.ApplyTrade(ctx, trade2, "buyer", "s2")

	pos, _ := svc.GetPosition(ctx, "buyer", "AAPL")
	if pos.AverageCostMinorUnits != 110 {
		t.Errorf("avg cost: want 110, got %d", pos.AverageCostMinorUnits)
	}
	if pos.QuantityUnits != 20 {
		t.Errorf("qty: want 20, got %d", pos.QuantityUnits)
	}
}

func TestGetPosition_NotFound(t *testing.T) {
	svc := positions.NewInMemoryPositionService()
	_, err := svc.GetPosition(context.Background(), "nobody", "AAPL")
	if !errors.Is(err, domain.ErrPositionNotFound) {
		t.Errorf("expected ErrPositionNotFound, got %v", err)
	}
}

func TestListByOwner(t *testing.T) {
	svc := positions.NewInMemoryPositionService()
	ctx := context.Background()
	_ = svc.ApplyTrade(ctx, makeTrade(5, 100), "alice", "bob")
	trade2 := domain.Trade{ID: "t2", InstrumentID: "GOOG", PriceMinorUnits: 200, QuantityUnits: 2, ExecutedAt: time.Now().UTC()}
	_ = svc.ApplyTrade(ctx, trade2, "alice", "bob")

	ps, err := svc.ListByOwner(ctx, "alice")
	if err != nil {
		t.Fatalf("ListByOwner: %v", err)
	}
	if len(ps) != 2 {
		t.Errorf("expected 2 positions for alice, got %d", len(ps))
	}
}

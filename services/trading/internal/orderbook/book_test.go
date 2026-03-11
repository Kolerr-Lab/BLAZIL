package orderbook_test

import (
	"testing"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/orderbook"
)

func makeOrder(id string, side domain.Side, price, qty int64) *domain.Order {
	now := time.Now().UTC()
	return &domain.Order{
		ID:                   domain.OrderID(id),
		InstrumentID:         "AAPL",
		OwnerID:              "owner-1",
		Side:                 side,
		LimitPriceMinorUnits: price,
		QuantityUnits:        qty,
		Status:               domain.OrderStatusOpen,
		PlacedAt:             now,
		UpdatedAt:            now,
	}
}

func TestBook_AddBidAndAsk(t *testing.T) {
	b := orderbook.New("AAPL")
	if err := b.Add(makeOrder("b1", domain.SideBuy, 100, 10)); err != nil {
		t.Fatalf("Add bid: %v", err)
	}
	if err := b.Add(makeOrder("a1", domain.SideSell, 102, 5)); err != nil {
		t.Fatalf("Add ask: %v", err)
	}
	if bid, ok := b.BestBid(); !ok || bid != 100 {
		t.Errorf("BestBid: got (%d,%v), want (100,true)", bid, ok)
	}
	if ask, ok := b.BestAsk(); !ok || ask != 102 {
		t.Errorf("BestAsk: got (%d,%v), want (102,true)", ask, ok)
	}
}

func TestBook_BidsDescending(t *testing.T) {
	b := orderbook.New("AAPL")
	_ = b.Add(makeOrder("b1", domain.SideBuy, 100, 10))
	_ = b.Add(makeOrder("b2", domain.SideBuy, 105, 5))
	_ = b.Add(makeOrder("b3", domain.SideBuy, 103, 3))

	levels := b.BidLevels()
	if len(levels) != 3 {
		t.Fatalf("expected 3 bid levels, got %d", len(levels))
	}
	if levels[0].PriceMinorUnits != 105 {
		t.Errorf("level[0]: want 105, got %d", levels[0].PriceMinorUnits)
	}
	if levels[1].PriceMinorUnits != 103 {
		t.Errorf("level[1]: want 103, got %d", levels[1].PriceMinorUnits)
	}
	if levels[2].PriceMinorUnits != 100 {
		t.Errorf("level[2]: want 100, got %d", levels[2].PriceMinorUnits)
	}
}

func TestBook_AsksAscending(t *testing.T) {
	b := orderbook.New("AAPL")
	_ = b.Add(makeOrder("a1", domain.SideSell, 105, 10))
	_ = b.Add(makeOrder("a2", domain.SideSell, 102, 5))
	_ = b.Add(makeOrder("a3", domain.SideSell, 103, 3))

	levels := b.AskLevels()
	if len(levels) != 3 {
		t.Fatalf("expected 3 ask levels, got %d", len(levels))
	}
	if levels[0].PriceMinorUnits != 102 {
		t.Errorf("level[0]: want 102, got %d", levels[0].PriceMinorUnits)
	}
	if levels[2].PriceMinorUnits != 105 {
		t.Errorf("level[2]: want 105, got %d", levels[2].PriceMinorUnits)
	}
}

func TestBook_SamePriceFIFO(t *testing.T) {
	b := orderbook.New("AAPL")
	_ = b.Add(makeOrder("b1", domain.SideBuy, 100, 5))
	_ = b.Add(makeOrder("b2", domain.SideBuy, 100, 7))

	levels := b.BidLevels()
	if len(levels) != 1 {
		t.Fatalf("expected 1 level, got %d", len(levels))
	}
	if levels[0].TotalUnits != 12 {
		t.Errorf("TotalUnits: want 12, got %d", levels[0].TotalUnits)
	}
	if levels[0].OrderCount != 2 {
		t.Errorf("OrderCount: want 2, got %d", levels[0].OrderCount)
	}
}

func TestBook_Cancel(t *testing.T) {
	b := orderbook.New("AAPL")
	_ = b.Add(makeOrder("b1", domain.SideBuy, 100, 10))
	if err := b.Cancel("b1"); err != nil {
		t.Fatalf("Cancel: %v", err)
	}
	if _, ok := b.BestBid(); ok {
		t.Error("expected no bids after cancel")
	}
}

func TestBook_CancelNotFound(t *testing.T) {
	b := orderbook.New("AAPL")
	err := b.Cancel("missing")
	if err != domain.ErrOrderNotFound {
		t.Errorf("expected ErrOrderNotFound, got %v", err)
	}
}

func TestBook_DuplicateAdd(t *testing.T) {
	b := orderbook.New("AAPL")
	_ = b.Add(makeOrder("b1", domain.SideBuy, 100, 10))
	err := b.Add(makeOrder("b1", domain.SideBuy, 100, 5))
	if err != domain.ErrOrderAlreadyExists {
		t.Errorf("expected ErrOrderAlreadyExists, got %v", err)
	}
}

func TestBook_EmptyBook(t *testing.T) {
	b := orderbook.New("AAPL")
	if _, ok := b.BestBid(); ok {
		t.Error("empty book should have no best bid")
	}
	if _, ok := b.BestAsk(); ok {
		t.Error("empty book should have no best ask")
	}
}

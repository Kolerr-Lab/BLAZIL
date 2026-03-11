package matching_test

import (
	"fmt"
	"testing"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orderbook"
)

var tradeCounter int

func nextTradeID() domain.TradeID {
	tradeCounter++
	return domain.TradeID(fmt.Sprintf("trade-%d", tradeCounter))
}

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

func newBook() *orderbook.Book {
	tradeCounter = 0
	return orderbook.New("AAPL")
}

func TestFIFO_NoMatch_BidBelowAsk(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()
	_ = book.Add(makeOrder("a1", domain.SideSell, 110, 10))

	taker := makeOrder("b1", domain.SideBuy, 100, 5)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 0 {
		t.Errorf("expected no trades, got %d", len(result.Trades))
	}
	if taker.Status != domain.OrderStatusOpen {
		t.Errorf("taker status: want open, got %s", taker.Status)
	}
}

func TestFIFO_FullFill_SingleMaker(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()
	maker := makeOrder("a1", domain.SideSell, 100, 10)
	_ = book.Add(maker)

	taker := makeOrder("b1", domain.SideBuy, 100, 10)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(result.Trades))
	}
	trade := result.Trades[0]
	if trade.QuantityUnits != 10 {
		t.Errorf("trade qty: want 10, got %d", trade.QuantityUnits)
	}
	if trade.PriceMinorUnits != 100 {
		t.Errorf("trade price: want 100 (maker price), got %d", trade.PriceMinorUnits)
	}
	if taker.Status != domain.OrderStatusFilled {
		t.Errorf("taker: want filled, got %s", taker.Status)
	}
	if maker.Status != domain.OrderStatusFilled {
		t.Errorf("maker: want filled, got %s", maker.Status)
	}
}

func TestFIFO_PartialFill_TakerLarger(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()
	maker := makeOrder("a1", domain.SideSell, 100, 5)
	_ = book.Add(maker)

	taker := makeOrder("b1", domain.SideBuy, 100, 10)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(result.Trades))
	}
	if result.Trades[0].QuantityUnits != 5 {
		t.Errorf("trade qty: want 5, got %d", result.Trades[0].QuantityUnits)
	}
	if taker.Status != domain.OrderStatusPartial {
		t.Errorf("taker: want partial, got %s", taker.Status)
	}
	if taker.FilledUnits != 5 {
		t.Errorf("taker filled: want 5, got %d", taker.FilledUnits)
	}
}

func TestFIFO_FIFOWithinLevel(t *testing.T) {
	// Two asks at price 100; first placed should be matched first.
	eng := matching.NewFIFOEngine()
	book := newBook()
	first := makeOrder("a1", domain.SideSell, 100, 5)
	second := makeOrder("a2", domain.SideSell, 100, 5)
	_ = book.Add(first)
	_ = book.Add(second)

	taker := makeOrder("b1", domain.SideBuy, 100, 5)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(result.Trades))
	}
	if result.Trades[0].MakerOrderID != "a1" {
		t.Errorf("expected first order (a1) to be matched first, got %s", result.Trades[0].MakerOrderID)
	}
	// second order should still be in the book
	if _, ok := book.BestAsk(); !ok {
		t.Error("expected second order to remain in book")
	}
}

func TestFIFO_MultipleMakers(t *testing.T) {
	// Taker fills across two price levels.
	eng := matching.NewFIFOEngine()
	book := newBook()
	_ = book.Add(makeOrder("a1", domain.SideSell, 100, 3))
	_ = book.Add(makeOrder("a2", domain.SideSell, 101, 3))

	taker := makeOrder("b1", domain.SideBuy, 105, 6)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 2 {
		t.Fatalf("expected 2 trades, got %d", len(result.Trades))
	}
	if taker.Status != domain.OrderStatusFilled {
		t.Errorf("taker: want filled, got %s", taker.Status)
	}
	// First trade at price 100 (best ask).
	if result.Trades[0].PriceMinorUnits != 100 {
		t.Errorf("trade[0] price: want 100, got %d", result.Trades[0].PriceMinorUnits)
	}
	if result.Trades[1].PriceMinorUnits != 101 {
		t.Errorf("trade[1] price: want 101, got %d", result.Trades[1].PriceMinorUnits)
	}
}

func TestFIFO_SellTaker_MatchesBid(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()
	maker := makeOrder("b1", domain.SideBuy, 100, 8)
	_ = book.Add(maker)

	taker := makeOrder("s1", domain.SideSell, 100, 8)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(result.Trades))
	}
	if result.Trades[0].QuantityUnits != 8 {
		t.Errorf("trade qty: want 8, got %d", result.Trades[0].QuantityUnits)
	}
}

func TestFIFO_SellTaker_NoMatchBelowLimit(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()
	_ = book.Add(makeOrder("b1", domain.SideBuy, 95, 10))

	taker := makeOrder("s1", domain.SideSell, 100, 5)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 0 {
		t.Errorf("expected no trades, got %d", len(result.Trades))
	}
}

func TestFIFO_TradePriceIsMakerPrice(t *testing.T) {
	// Taker buys at 110, maker asks at 100 — execution at maker price 100.
	eng := matching.NewFIFOEngine()
	book := newBook()
	_ = book.Add(makeOrder("a1", domain.SideSell, 100, 5))

	taker := makeOrder("b1", domain.SideBuy, 110, 5)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(result.Trades))
	}
	if result.Trades[0].PriceMinorUnits != 100 {
		t.Errorf("trade price: want 100 (maker), got %d", result.Trades[0].PriceMinorUnits)
	}
}

func TestFIFO_EmptyBook(t *testing.T) {
	eng := matching.NewFIFOEngine()
	book := newBook()

	taker := makeOrder("b1", domain.SideBuy, 100, 5)
	result := eng.Match(book, taker, nextTradeID)

	if len(result.Trades) != 0 {
		t.Errorf("expected no trades on empty book, got %d", len(result.Trades))
	}
	if taker.Status != domain.OrderStatusOpen {
		t.Errorf("taker: want open, got %s", taker.Status)
	}
}

package orders_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orders"
)

func newSvc() *orders.InMemoryOrderService {
	return orders.NewInMemoryOrderService(matching.NewFIFOEngine())
}

func buyReq(id string, price, qty int64) orders.PlaceOrderRequest {
	return orders.PlaceOrderRequest{
		ID:                   domain.OrderID(id),
		InstrumentID:         "AAPL",
		OwnerID:              "owner-1",
		Side:                 domain.SideBuy,
		LimitPriceMinorUnits: price,
		QuantityUnits:        qty,
	}
}

func sellReq(id string, price, qty int64) orders.PlaceOrderRequest {
	return orders.PlaceOrderRequest{
		ID:                   domain.OrderID(id),
		InstrumentID:         "AAPL",
		OwnerID:              "owner-2",
		Side:                 domain.SideSell,
		LimitPriceMinorUnits: price,
		QuantityUnits:        qty,
	}
}

func TestPlaceOrder_RestingOrder(t *testing.T) {
	svc := newSvc()
	order, trades, err := svc.PlaceOrder(context.Background(), buyReq("b1", 100, 10))
	if err != nil {
		t.Fatalf("PlaceOrder: %v", err)
	}
	if len(trades) != 0 {
		t.Errorf("expected no trades, got %d", len(trades))
	}
	if order.Status != domain.OrderStatusOpen {
		t.Errorf("status: want open, got %s", order.Status)
	}
}

func TestPlaceOrder_ImmediateFill(t *testing.T) {
	svc := newSvc()
	ctx := context.Background()
	_, _, _ = svc.PlaceOrder(ctx, sellReq("a1", 100, 10))
	order, trades, err := svc.PlaceOrder(ctx, buyReq("b1", 100, 10))
	if err != nil {
		t.Fatalf("PlaceOrder: %v", err)
	}
	if len(trades) != 1 {
		t.Fatalf("expected 1 trade, got %d", len(trades))
	}
	if order.Status != domain.OrderStatusFilled {
		t.Errorf("status: want filled, got %s", order.Status)
	}
}

func TestPlaceOrder_DuplicateID(t *testing.T) {
	svc := newSvc()
	ctx := context.Background()
	_, _, _ = svc.PlaceOrder(ctx, buyReq("b1", 100, 10))
	_, _, err := svc.PlaceOrder(ctx, buyReq("b1", 100, 5))
	if !errors.Is(err, domain.ErrOrderAlreadyExists) {
		t.Errorf("expected ErrOrderAlreadyExists, got %v", err)
	}
}

func TestCancelOrder(t *testing.T) {
	svc := newSvc()
	ctx := context.Background()
	_, _, _ = svc.PlaceOrder(ctx, buyReq("b1", 100, 10))
	if err := svc.CancelOrder(ctx, "b1"); err != nil {
		t.Fatalf("CancelOrder: %v", err)
	}
	o, _ := svc.GetOrder(ctx, "b1")
	if o.Status != domain.OrderStatusCancelled {
		t.Errorf("status: want cancelled, got %s", o.Status)
	}
}

func TestGetOrder_NotFound(t *testing.T) {
	svc := newSvc()
	_, err := svc.GetOrder(context.Background(), "missing")
	if !errors.Is(err, domain.ErrOrderNotFound) {
		t.Errorf("expected ErrOrderNotFound, got %v", err)
	}
}

// Package orders implements order lifecycle management for the trading service.
package orders

import (
	"context"
	"fmt"
	"strings"
	"sync/atomic"
	"time"

	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orderbook"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// PgOrderService is a PostgreSQL-backed OrderService.
// It persists orders and trades durably; the in-process order book is rebuilt
// from open/partial orders on startup.
type PgOrderService struct {
	db      *pgxpool.Pool
	engine  matching.Engine
	tradeID atomic.Int64
	mem     *InMemoryOrderService
}

// NewPgOrderService creates a PgOrderService and rebuilds the in-memory order
// book from all open/partial orders in the database.
func NewPgOrderService(ctx context.Context, db *pgxpool.Pool, engine matching.Engine) (*PgOrderService, error) {
	svc := &PgOrderService{
		db:     db,
		engine: engine,
		mem:    NewInMemoryOrderService(engine),
	}
	if err := svc.rebuildFromDB(ctx); err != nil {
		return nil, fmt.Errorf("PgOrderService rebuild: %w", err)
	}
	return svc, nil
}

func (s *PgOrderService) rebuildFromDB(ctx context.Context) error {
	const q = `
		SELECT id, instrument_id, owner_id, side, limit_price_minor_units,
		       quantity_units, filled_units, status, placed_at, updated_at
		FROM orders
		WHERE status IN (1, 2)
		ORDER BY placed_at ASC`

	rows, err := s.db.Query(ctx, q)
	if err != nil {
		return err
	}
	defer rows.Close()

	for rows.Next() {
		var o domain.Order
		if err := rows.Scan(
			&o.ID, &o.InstrumentID, &o.OwnerID, &o.Side,
			&o.LimitPriceMinorUnits, &o.QuantityUnits,
			&o.FilledUnits, &o.Status,
			&o.PlacedAt, &o.UpdatedAt,
		); err != nil {
			return err
		}
		s.mem.mu.Lock()
		s.mem.orders[o.ID] = &o
		book := s.mem.bookForInstrument(o.InstrumentID)
		_ = book.Add(&o)
		s.mem.mu.Unlock()
	}
	return rows.Err()
}

func (s *PgOrderService) nextTradeID() domain.TradeID {
	n := s.tradeID.Add(1)
	return domain.TradeID(fmt.Sprintf("trade-%d", n))
}

// PlaceOrder persists the order, runs matching, persists trades, and updates
// filled quantities — all inside a single database transaction.
func (s *PgOrderService) PlaceOrder(ctx context.Context, req PlaceOrderRequest) (*domain.Order, []domain.Trade, error) {
	if req.QuantityUnits <= 0 {
		return nil, nil, domain.ErrInvalidQuantity
	}
	if req.LimitPriceMinorUnits <= 0 {
		return nil, nil, domain.ErrInvalidPrice
	}
	if req.Side != domain.SideBuy && req.Side != domain.SideSell {
		return nil, nil, domain.ErrUnknownSide
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

	dbTx, err := s.db.Begin(ctx)
	if err != nil {
		return nil, nil, fmt.Errorf("begin tx: %w", err)
	}
	defer dbTx.Rollback(ctx) //nolint:errcheck

	const insertOrder = `
		INSERT INTO orders
			(id, instrument_id, owner_id, side, limit_price_minor_units,
			 quantity_units, filled_units, status, placed_at, updated_at)
		VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`

	if _, err := dbTx.Exec(ctx, insertOrder,
		string(order.ID), string(order.InstrumentID), order.OwnerID, int(order.Side),
		order.LimitPriceMinorUnits, order.QuantityUnits,
		order.FilledUnits, int(order.Status),
		order.PlacedAt, order.UpdatedAt,
	); err != nil {
		if strings.Contains(err.Error(), "23505") {
			return nil, nil, domain.ErrOrderAlreadyExists
		}
		return nil, nil, fmt.Errorf("insert order: %w", err)
	}

	// Run matching in-memory.
	s.mem.mu.Lock()
	book := s.mem.bookForInstrument(order.InstrumentID)
	result := s.engine.Match(book, order, s.nextTradeID)
	if order.Status != domain.OrderStatusFilled {
		_ = book.Add(order)
	}
	s.mem.orders[order.ID] = order
	s.mem.mu.Unlock()

	// Persist trades.
	for _, t := range result.Trades {
		const q = `
			INSERT INTO trades
				(id, instrument_id, maker_order_id, taker_order_id,
				 price_minor_units, quantity_units, executed_at)
			VALUES ($1,$2,$3,$4,$5,$6,$7)`
		if _, err := dbTx.Exec(ctx, q,
			string(t.ID), string(t.InstrumentID),
			string(t.MakerOrderID), string(t.TakerOrderID),
			t.PriceMinorUnits, t.QuantityUnits, t.ExecutedAt,
		); err != nil {
			return nil, nil, fmt.Errorf("insert trade: %w", err)
		}
	}

	// Flush updated maker orders to DB.
	for _, maker := range result.FilledMakers {
		if _, err := dbTx.Exec(ctx,
			`UPDATE orders SET filled_units = $1, status = $2, updated_at = $3 WHERE id = $4`,
			maker.FilledUnits, int(maker.Status), maker.UpdatedAt, string(maker.ID),
		); err != nil {
			return nil, nil, fmt.Errorf("update maker order: %w", err)
		}
	}

	// Flush taker if it received any fills.
	if order.FilledUnits > 0 {
		if _, err := dbTx.Exec(ctx,
			`UPDATE orders SET filled_units = $1, status = $2, updated_at = $3 WHERE id = $4`,
			order.FilledUnits, int(order.Status), order.UpdatedAt, string(order.ID),
		); err != nil {
			return nil, nil, fmt.Errorf("update taker order: %w", err)
		}
	}

	if err := dbTx.Commit(ctx); err != nil {
		return nil, nil, fmt.Errorf("commit: %w", err)
	}

	trades := make([]domain.Trade, len(result.Trades))
	for i, t := range result.Trades {
		trades[i] = *t
	}
	return order, trades, nil
}

// CancelOrder marks an order cancelled in DB and removes it from the in-memory book.
func (s *PgOrderService) CancelOrder(ctx context.Context, id domain.OrderID) error {
	tag, err := s.db.Exec(ctx,
		`UPDATE orders SET status = $1, updated_at = now() WHERE id = $2 AND status IN (1,2)`,
		int(domain.OrderStatusCancelled), string(id))
	if err != nil {
		return fmt.Errorf("cancel order: %w", err)
	}
	if tag.RowsAffected() == 0 {
		return domain.ErrOrderNotFound
	}
	return s.mem.CancelOrder(ctx, id)
}

// GetOrder returns a single order by ID.
func (s *PgOrderService) GetOrder(ctx context.Context, id domain.OrderID) (*domain.Order, error) {
	const q = `
		SELECT id, instrument_id, owner_id, side, limit_price_minor_units,
		       quantity_units, filled_units, status, placed_at, updated_at
		FROM orders WHERE id = $1`

	var o domain.Order
	err := s.db.QueryRow(ctx, q, string(id)).Scan(
		&o.ID, &o.InstrumentID, &o.OwnerID, &o.Side,
		&o.LimitPriceMinorUnits, &o.QuantityUnits,
		&o.FilledUnits, &o.Status,
		&o.PlacedAt, &o.UpdatedAt,
	)
	if err != nil {
		if err == pgx.ErrNoRows {
			return nil, domain.ErrOrderNotFound
		}
		return nil, fmt.Errorf("get order: %w", err)
	}
	return &o, nil
}

// ListByOwner returns all orders for the given owner, newest first.
func (s *PgOrderService) ListByOwner(ctx context.Context, ownerID string) ([]*domain.Order, error) {
	const q = `
		SELECT id, instrument_id, owner_id, side, limit_price_minor_units,
		       quantity_units, filled_units, status, placed_at, updated_at
		FROM orders WHERE owner_id = $1 ORDER BY placed_at DESC`

	rows, err := s.db.Query(ctx, q, ownerID)
	if err != nil {
		return nil, fmt.Errorf("list orders: %w", err)
	}
	defer rows.Close()

	var out []*domain.Order
	for rows.Next() {
		var o domain.Order
		if err := rows.Scan(
			&o.ID, &o.InstrumentID, &o.OwnerID, &o.Side,
			&o.LimitPriceMinorUnits, &o.QuantityUnits,
			&o.FilledUnits, &o.Status,
			&o.PlacedAt, &o.UpdatedAt,
		); err != nil {
			return nil, err
		}
		out = append(out, &o)
	}
	return out, rows.Err()
}

// compile-time interface check.
var _ OrderService = (*PgOrderService)(nil)

// prevent unused import of orderbook.
var _ = (*orderbook.Book)(nil)

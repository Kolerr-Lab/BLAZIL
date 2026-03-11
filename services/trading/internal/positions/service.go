// Package positions manages owner positions updated on trade settlement.
package positions

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/trading/internal/domain"
)

// PositionService manages per-owner, per-instrument positions.
type PositionService interface {
	// ApplyTrade updates positions for buyer and seller based on a settled trade.
	ApplyTrade(ctx context.Context, trade domain.Trade, buyerOwnerID, sellerOwnerID string) error

	// GetPosition returns the current position for (ownerID, instrumentID).
	// Returns ErrPositionNotFound if no position exists.
	GetPosition(ctx context.Context, ownerID string, instrumentID domain.InstrumentID) (*domain.Position, error)

	// ListByOwner returns all positions for the given owner.
	ListByOwner(ctx context.Context, ownerID string) ([]*domain.Position, error)
}

// InMemoryPositionService is a thread-safe in-memory PositionService.
type InMemoryPositionService struct {
	mu        sync.RWMutex
	positions map[positionKey]*domain.Position
	posID     int64
}

type positionKey struct {
	ownerID      string
	instrumentID domain.InstrumentID
}

// NewInMemoryPositionService constructs an empty InMemoryPositionService.
func NewInMemoryPositionService() *InMemoryPositionService {
	return &InMemoryPositionService{
		positions: make(map[positionKey]*domain.Position),
	}
}

// ApplyTrade implements PositionService.
// Buyer's position increases by trade.QuantityUnits; seller's position decreases.
// Average cost is recomputed using a volume-weighted average.
func (s *InMemoryPositionService) ApplyTrade(_ context.Context, trade domain.Trade, buyerOwnerID, sellerOwnerID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	now := time.Now().UTC()

	// Update buyer (long position increases).
	buyerPos := s.getOrCreate(buyerOwnerID, trade.InstrumentID, now)
	buyerPos.AverageCostMinorUnits = weightedAvg(
		buyerPos.AverageCostMinorUnits, buyerPos.QuantityUnits,
		trade.PriceMinorUnits, trade.QuantityUnits,
	)
	buyerPos.QuantityUnits += trade.QuantityUnits
	buyerPos.UpdatedAt = now

	// Update seller (long position decreases; short not supported in v1).
	sellerPos := s.getOrCreate(sellerOwnerID, trade.InstrumentID, now)
	sellerPos.QuantityUnits -= trade.QuantityUnits
	sellerPos.UpdatedAt = now

	return nil
}

// GetPosition implements PositionService.
func (s *InMemoryPositionService) GetPosition(_ context.Context, ownerID string, instrumentID domain.InstrumentID) (*domain.Position, error) {
	s.mu.RLock()
	p, ok := s.positions[positionKey{ownerID, instrumentID}]
	s.mu.RUnlock()
	if !ok {
		return nil, fmt.Errorf("position for owner %s instrument %s: %w", ownerID, instrumentID, domain.ErrPositionNotFound)
	}
	return p, nil
}

// ListByOwner implements PositionService.
func (s *InMemoryPositionService) ListByOwner(_ context.Context, ownerID string) ([]*domain.Position, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var result []*domain.Position
	for k, p := range s.positions {
		if k.ownerID == ownerID {
			result = append(result, p)
		}
	}
	return result, nil
}

// getOrCreate returns the existing position or creates a zero position.
// Must be called with s.mu held (write).
func (s *InMemoryPositionService) getOrCreate(ownerID string, instrumentID domain.InstrumentID, now time.Time) *domain.Position {
	key := positionKey{ownerID, instrumentID}
	p, ok := s.positions[key]
	if !ok {
		s.posID++
		p = &domain.Position{
			ID:           domain.PositionID(fmt.Sprintf("pos-%d", s.posID)),
			OwnerID:      ownerID,
			InstrumentID: instrumentID,
			UpdatedAt:    now,
		}
		s.positions[key] = p
	}
	return p
}

// weightedAvg computes the new VWAP after adding (newPrice, newQty) to (oldAvg, oldQty).
// Uses integer arithmetic: (oldAvg*oldQty + newPrice*newQty) / (oldQty + newQty).
// Returns 0 if total quantity would be zero.
func weightedAvg(oldAvg, oldQty, newPrice, newQty int64) int64 {
	total := oldQty + newQty
	if total == 0 {
		return 0
	}
	return (oldAvg*oldQty + newPrice*newQty) / total
}

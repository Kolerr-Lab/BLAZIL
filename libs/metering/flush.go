package metering

import (
	"context"
	"fmt"
	"time"

	"go.uber.org/zap"
)

// UsageRow represents a single metering record to be persisted.
type UsageRow struct {
	TenantID    string
	WindowStart time.Time
	WindowEnd   time.Time
	TxCount     int64
}

// UsageWriter persists metering rows to durable storage.
// Implementations must be goroutine-safe.
type UsageWriter interface {
	// UpsertUsage inserts or accumulates rows. For each (tenant_id, window_start)
	// pair, tx_count is added to any existing row (ON CONFLICT DO UPDATE).
	UpsertUsage(ctx context.Context, rows []UsageRow) error

	// MonthlyTotal returns the sum of confirmed tx counts for a tenant within
	// the billing month that contains t (UTC). Used for pricing tier selection.
	MonthlyTotal(ctx context.Context, tenantID string, t time.Time) (int64, error)

	// WindowedUsage returns ordered (window_start ASC) usage rows for a tenant
	// within the calendar month of year/month (1-based). Used for invoice generation.
	WindowedUsage(ctx context.Context, tenantID string, year, month int) ([]UsageRow, error)
}

// Flusher periodically drains the Recorder and writes deltas to a UsageWriter.
//
// Delivery guarantee: at-least-once.
// On UsageWriter failure the counts are re-injected into the Recorder so that
// they are retried on the next tick. No data is silently dropped.
type Flusher struct {
	rec    Recorder
	writer UsageWriter
	logger *zap.Logger
}

// NewFlusher creates a Flusher that flushes rec into writer every WindowSize.
func NewFlusher(rec Recorder, writer UsageWriter, logger *zap.Logger) *Flusher {
	return &Flusher{rec: rec, writer: writer, logger: logger}
}

// Run starts the flush loop. It blocks until ctx is cancelled.
// Launch in a goroutine: go flusher.Run(ctx)
//
// A final best-effort flush is attempted when ctx is cancelled.
func (f *Flusher) Run(ctx context.Context) {
	ticker := time.NewTicker(WindowSize)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			if err := f.flushAt(shutdownCtx, time.Now().UTC().Truncate(WindowSize)); err != nil {
				f.logger.Error("metering: final flush on shutdown failed", zap.Error(err))
			}
			cancel()
			return
		case tick := <-ticker.C:
			if err := f.flushAt(ctx, tick.UTC().Truncate(WindowSize)); err != nil {
				f.logger.Error("metering: periodic flush failed", zap.Error(err))
			}
		}
	}
}

// Flush drains the recorder immediately.
// Used by graceful-shutdown hooks and tests.
func (f *Flusher) Flush(ctx context.Context) error {
	return f.flushAt(ctx, time.Now().UTC().Truncate(WindowSize))
}

func (f *Flusher) flushAt(ctx context.Context, windowStart time.Time) error {
	counts := f.rec.Snapshot()
	if len(counts) == 0 {
		return nil
	}

	windowEnd := windowStart.Add(WindowSize)
	rows := make([]UsageRow, 0, len(counts))
	for tenantID, n := range counts {
		rows = append(rows, UsageRow{
			TenantID:    tenantID,
			WindowStart: windowStart,
			WindowEnd:   windowEnd,
			TxCount:     n,
		})
	}

	if err := f.writer.UpsertUsage(ctx, rows); err != nil {
		// Re-inject counts so they survive until the next tick.
		for _, row := range rows {
			f.rec.Record(row.TenantID, row.TxCount)
		}
		return fmt.Errorf("metering flush: %w", err)
	}

	f.logger.Debug("metering: flushed usage",
		zap.Int("tenants", len(rows)),
		zap.Time("window_start", windowStart),
	)
	return nil
}

package metering_test

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"

	"go.uber.org/zap"

	"github.com/blazil/metering"
)

// ── Recorder ─────────────────────────────────────────────────────────────────

func TestRecorder_RecordAndSnapshot(t *testing.T) {
	rec := metering.NewRecorder()

	rec.Record("tenant-a", 3)
	rec.Record("tenant-b", 1)
	rec.Record("tenant-a", 2)

	snap := rec.Snapshot()

	if snap["tenant-a"] != 5 {
		t.Errorf("tenant-a: want 5, got %d", snap["tenant-a"])
	}
	if snap["tenant-b"] != 1 {
		t.Errorf("tenant-b: want 1, got %d", snap["tenant-b"])
	}

	// Second snapshot must be empty (counters drained).
	snap2 := rec.Snapshot()
	if len(snap2) != 0 {
		t.Errorf("snapshot after drain: expected empty, got %v", snap2)
	}
}

func TestRecorder_ZeroCountsOmitted(t *testing.T) {
	rec := metering.NewRecorder()
	// Never record anything for this tenant → must not appear in snapshot.
	snap := rec.Snapshot()
	if len(snap) != 0 {
		t.Errorf("expected empty snapshot, got %v", snap)
	}
}

func TestRecorder_Concurrent(t *testing.T) {
	const (
		goroutines = 64
		increments = 1_000
	)
	rec := metering.NewRecorder()
	var wg sync.WaitGroup
	wg.Add(goroutines)
	for i := 0; i < goroutines; i++ {
		go func() {
			defer wg.Done()
			for j := 0; j < increments; j++ {
				rec.Record("tenant-concurrent", 1)
			}
		}()
	}
	wg.Wait()

	snap := rec.Snapshot()
	want := int64(goroutines * increments)
	if snap["tenant-concurrent"] != want {
		t.Errorf("concurrent: want %d, got %d", want, snap["tenant-concurrent"])
	}
}

func TestRecorder_ManyTenants(t *testing.T) {
	rec := metering.NewRecorder()
	// Ensure FNV-1a sharding distributes across all 64 shards.
	const n = 256
	for i := 0; i < n; i++ {
		rec.Record(fmt.Sprintf("tenant-%03d", i), int64(i+1))
	}
	snap := rec.Snapshot()
	if len(snap) != n {
		t.Errorf("expected %d tenants in snapshot, got %d", n, len(snap))
	}
	for i := 0; i < n; i++ {
		key := fmt.Sprintf("tenant-%03d", i)
		if snap[key] != int64(i+1) {
			t.Errorf("%s: want %d, got %d", key, i+1, snap[key])
		}
	}
}

// ── Flusher ───────────────────────────────────────────────────────────────────

type mockWriter struct {
	mu     sync.Mutex
	rows   []metering.UsageRow
	err    error // if non-nil, UpsertUsage returns this error
	calls  int
}

func (m *mockWriter) UpsertUsage(_ context.Context, rows []metering.UsageRow) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.calls++
	if m.err != nil {
		return m.err
	}
	m.rows = append(m.rows, rows...)
	return nil
}

func (m *mockWriter) MonthlyTotal(_ context.Context, _ string, _ time.Time) (int64, error) {
	return 0, nil
}

func (m *mockWriter) WindowedUsage(_ context.Context, _ string, _, _ int) ([]metering.UsageRow, error) {
	return nil, nil
}

func (m *mockWriter) Rows() []metering.UsageRow {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([]metering.UsageRow, len(m.rows))
	copy(out, m.rows)
	return out
}

func TestFlusher_FlushWritesRows(t *testing.T) {
	rec := metering.NewRecorder()
	rec.Record("t1", 10)
	rec.Record("t2", 20)

	w := &mockWriter{}
	f := metering.NewFlusher(rec, w, nopLogger())
	if err := f.Flush(context.Background()); err != nil {
		t.Fatalf("unexpected flush error: %v", err)
	}

	rows := w.Rows()
	if len(rows) != 2 {
		t.Fatalf("expected 2 rows, got %d", len(rows))
	}

	total := int64(0)
	for _, r := range rows {
		total += r.TxCount
	}
	if total != 30 {
		t.Errorf("expected total tx_count 30, got %d", total)
	}
}

func TestFlusher_ReInjectsOnWriteFailure(t *testing.T) {
	rec := metering.NewRecorder()
	rec.Record("t1", 5)

	w := &mockWriter{err: errors.New("db unavailable")}
	f := metering.NewFlusher(rec, w, nopLogger())
	if err := f.Flush(context.Background()); err == nil {
		t.Fatal("expected flush error, got nil")
	}

	// Count must be re-injected into the recorder.
	snap := rec.Snapshot()
	if snap["t1"] != 5 {
		t.Errorf("expected re-injected count 5, got %d", snap["t1"])
	}
}

func TestFlusher_EmptySnapshotSkipsWrite(t *testing.T) {
	rec := metering.NewRecorder()
	w := &mockWriter{}
	f := metering.NewFlusher(rec, w, nopLogger())
	if err := f.Flush(context.Background()); err != nil {
		t.Fatalf("unexpected flush error: %v", err)
	}
	if w.calls != 0 {
		t.Errorf("expected 0 writer calls for empty snapshot, got %d", w.calls)
	}
}

// ── Pricing ───────────────────────────────────────────────────────────────────

func TestPricePerTxMicroUSD_Tiers(t *testing.T) {
	tests := []struct {
		name        string
		tier        metering.Tier
		cumulative  int64
		wantMicroSD int64
	}{
		{"free tier always 0", metering.TierFree, 0, 0},
		{"free tier non-zero cumulative still 0", metering.TierFree, 5_000_000, 0},
		{"enterprise always 0", metering.TierEnterprise, 0, 0},
		{"cloud saas tier-1 first tx", metering.TierCloudSaaS, 0, 1_000},
		{"cloud saas tier-1 at 999k", metering.TierCloudSaaS, 999_999, 1_000},
		{"cloud saas tier-2 at 1M", metering.TierCloudSaaS, 1_000_000, 500},
		{"cloud saas tier-2 at 9.9M", metering.TierCloudSaaS, 9_999_999, 500},
		{"cloud saas tier-3 at 10M", metering.TierCloudSaaS, 10_000_000, 200},
		{"cloud saas tier-3 at 99.9M", metering.TierCloudSaaS, 99_999_999, 200},
		{"cloud saas tier-4 at 100M", metering.TierCloudSaaS, 100_000_000, 100},
		{"cloud saas tier-4 at 1B", metering.TierCloudSaaS, 1_000_000_000, 100},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := metering.PricePerTxMicroUSD(tc.tier, tc.cumulative)
			if got != tc.wantMicroSD {
				t.Errorf("PricePerTxMicroUSD(%s, %d): want %d, got %d",
					tc.tier, tc.cumulative, tc.wantMicroSD, got)
			}
		})
	}
}

func TestCalculateInvoice_TierTransition(t *testing.T) {
	// 1.5M tx: first 1M at $0.001, next 500K at $0.0005.
	counts := []metering.WindowCount{
		{WindowStart: time.Date(2026, 6, 1, 0, 0, 0, 0, time.UTC), Count: 1_000_000},
		{WindowStart: time.Date(2026, 6, 1, 0, 1, 0, 0, time.UTC), Count: 500_000},
	}
	lines := metering.CalculateInvoice("tenant-x", metering.TierCloudSaaS, counts)
	if len(lines) != 2 {
		t.Fatalf("expected 2 lines, got %d", len(lines))
	}
	// Line 1: 1_000_000 × 1_000 µ$ = 1_000_000_000 µ$ = $1000
	if lines[0].TotalMicroUSD != 1_000_000_000 {
		t.Errorf("line 0 total: want 1_000_000_000, got %d", lines[0].TotalMicroUSD)
	}
	// Line 2: 500_000 × 500 µ$ = 250_000_000 µ$ = $250
	if lines[1].TotalMicroUSD != 250_000_000 {
		t.Errorf("line 1 total: want 250_000_000, got %d", lines[1].TotalMicroUSD)
	}
	total := metering.TotalInvoiceMicroUSD(lines)
	if total != 1_250_000_000 {
		t.Errorf("invoice total: want 1_250_000_000, got %d", total)
	}
}

func TestCalculateInvoice_EmptyIsNil(t *testing.T) {
	lines := metering.CalculateInvoice("t", metering.TierCloudSaaS, nil)
	if len(lines) != 0 {
		t.Errorf("expected empty lines for nil counts, got %d", len(lines))
	}
}

// ── helpers ───────────────────────────────────────────────────────────────────

func nopLogger() *zap.Logger {
	return zap.NewNop()
}

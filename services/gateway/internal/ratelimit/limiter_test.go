package ratelimit_test

import (
	"testing"

	"github.com/blazil/services/gateway/internal/ratelimit"
)

func TestLimiter_AllowsWithinBurst(t *testing.T) {
	l := ratelimit.NewLimiter()
	const (
		rps   = 10
		burst = 5
	)
	// The first 'burst' requests must all be allowed (full bucket).
	for i := 0; i < burst; i++ {
		if !l.Allow("tenant-a", rps, burst) {
			t.Errorf("request %d: expected Allow=true (within burst)", i+1)
		}
	}
}

func TestLimiter_DeniesAfterBurst(t *testing.T) {
	l := ratelimit.NewLimiter()
	const (
		rps   = 1 // 1 token/second — bucket refills slowly
		burst = 2
	)
	// Drain the bucket.
	for i := 0; i < burst; i++ {
		l.Allow("tenant-b", rps, burst)
	}
	// Next request must be denied (bucket empty, no time to refill).
	if l.Allow("tenant-b", rps, burst) {
		t.Error("expected Allow=false after draining burst bucket")
	}
}

func TestLimiter_IsolatesTenantsIndependently(t *testing.T) {
	l := ratelimit.NewLimiter()
	// Drain tenant-c completely.
	for i := 0; i < 5; i++ {
		l.Allow("tenant-c", 1, 5)
	}
	// tenant-d should still have a full bucket.
	if !l.Allow("tenant-d", 100, 100) {
		t.Error("tenant-d was throttled by tenant-c's exhausted budget")
	}
}

func TestLimiter_RecreatesOnConfigChange(t *testing.T) {
	l := ratelimit.NewLimiter()
	// Drain with original config (burst=1).
	l.Allow("tenant-e", 1, 1)
	if l.Allow("tenant-e", 1, 1) {
		t.Error("expected throttle after draining burst=1 bucket")
	}

	// After a plan upgrade (burst=10), the limiter must be recreated at full capacity.
	if !l.Allow("tenant-e", 1, 10) {
		t.Error("expected Allow=true after config change to burst=10")
	}
}

func TestLimiter_DeleteRemovesCachedLimiter(t *testing.T) {
	l := ratelimit.NewLimiter()
	// Drain.
	for i := 0; i < 3; i++ {
		l.Allow("tenant-f", 1, 3)
	}
	// Delete and re-check — bucket should be full again after recreation.
	l.Delete("tenant-f")
	if !l.Allow("tenant-f", 1, 3) {
		t.Error("expected Allow=true for fresh limiter after Delete")
	}
}

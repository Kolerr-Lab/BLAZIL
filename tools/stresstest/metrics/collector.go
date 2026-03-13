// Package metrics provides a thread-safe metrics collector for the Blazil
// stress test.  Hot-path counters use atomics; latency samples are drained
// by a background goroutine so workers never block.
package metrics

import (
	"math"
	"sort"
	"sync"
	"sync/atomic"
	"time"
)

// Sample is a point-in-time snapshot recorded every interval.
type Sample struct {
	Elapsed time.Duration
	TPS     float64
	P50Ms   float64
	P99Ms   float64
	ErrPct  float64
}

// Collector accumulates request counts and latencies.
// All exported methods are safe for concurrent use.
type Collector struct {
	total   atomic.Int64
	success atomic.Int64
	failed  atomic.Int64

	latCh chan int64 // nanoseconds; buffered drain channel

	mu     sync.Mutex
	latBuf []int64
}

// NewCollector creates a Collector and starts the background latency drainer.
// The returned stop function must be called to release resources.
func NewCollector() (*Collector, func()) {
	c := &Collector{
		latCh:  make(chan int64, 100_000),
		latBuf: make([]int64, 0, 200_000),
	}
	done := make(chan struct{})
	go func() {
		for {
			select {
			case ns := <-c.latCh:
				c.mu.Lock()
				c.latBuf = append(c.latBuf, ns)
				c.mu.Unlock()
			case <-done:
				// Drain remaining samples.
				for {
					select {
					case ns := <-c.latCh:
						c.mu.Lock()
						c.latBuf = append(c.latBuf, ns)
						c.mu.Unlock()
					default:
						return
					}
				}
			}
		}
	}()
	return c, func() { close(done) }
}

// Record registers one completed request.  err may be nil (success) or
// non-nil (failure).  ns is the wall-clock latency in nanoseconds.
func (c *Collector) Record(ns int64, err error) {
	c.total.Add(1)
	if err == nil {
		c.success.Add(1)
	} else {
		c.failed.Add(1)
	}
	select {
	case c.latCh <- ns:
	default:
		// Channel full; drop sample rather than stall the hot path.
	}
}

// Snapshot returns the current counters and drains the latency buffer.
// The latency buffer is reset after the snapshot.
func (c *Collector) Snapshot() (total, success, failed int64, p50Ms, p99Ms float64) {
	total = c.total.Load()
	success = c.success.Load()
	failed = c.failed.Load()

	c.mu.Lock()
	buf := make([]int64, len(c.latBuf))
	copy(buf, c.latBuf)
	c.latBuf = c.latBuf[:0]
	c.mu.Unlock()

	p50Ms, p99Ms = percentiles(buf)
	return
}

// SnapshotDelta returns the counts accumulated since the previous SnapshotDelta
// call (or since creation) and resets those counters to zero.  The latency
// buffer is also drained and reset.  Use this for per-interval TPS calculations.
func (c *Collector) SnapshotDelta() (total, success, failed int64, p50Ms, p99Ms float64) {
	total = c.total.Swap(0)
	success = c.success.Swap(0)
	failed = c.failed.Swap(0)

	c.mu.Lock()
	buf := make([]int64, len(c.latBuf))
	copy(buf, c.latBuf)
	c.latBuf = c.latBuf[:0]
	c.mu.Unlock()

	p50Ms, p99Ms = percentiles(buf)
	return
}

// Reset clears all counters and the latency buffer.  Call between scenarios.
func (c *Collector) Reset() {
	c.total.Store(0)
	c.success.Store(0)
	c.failed.Store(0)
	c.mu.Lock()
	c.latBuf = c.latBuf[:0]
	c.mu.Unlock()
}

// percentiles computes P50 and P99 from a slice of nanosecond latencies.
// Returns 0,0 for an empty slice.
func percentiles(ns []int64) (p50Ms, p99Ms float64) {
	if len(ns) == 0 {
		return 0, 0
	}
	sort.Slice(ns, func(i, j int) bool { return ns[i] < ns[j] })
	p50Ms = float64(ns[rank(ns, 0.50)]) / float64(time.Millisecond)
	p99Ms = float64(ns[rank(ns, 0.99)]) / float64(time.Millisecond)
	return
}

func rank(ns []int64, p float64) int {
	idx := int(math.Ceil(p*float64(len(ns)))) - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(ns) {
		idx = len(ns) - 1
	}
	return idx
}

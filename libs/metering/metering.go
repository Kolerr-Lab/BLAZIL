// Package metering provides per-tenant transaction counting for Blazil Cloud billing.
//
// Architecture:
//
//	Recorder — goroutine-safe, sharded atomic counters; one counter per tenant.
//	           Record() is on the hot path: called once per API request.
//	           Snapshot() drains all counters atomically; called by the Flusher.
//
//	Flusher  — background goroutine. Every WindowSize it calls Snapshot() and
//	           forwards deltas to a UsageWriter (typically Postgres). At-least-once
//	           delivery: on write failure counts are re-injected for the next tick.
//
//	Pricing  — pure functions. Given tier + cumulative monthly volume → price/tx.
//
// Source of truth: TigerBeetle ledger.
// Metering is a real-time billing approximation. End-of-month reconciliation
// against the TigerBeetle audit trail is required before finalising invoices.
package metering

import (
	"sync"
	"sync/atomic"
	"time"
)

// WindowSize is the metering flush cadence. One Postgres row is written
// per (tenant_id, window_start) pair per flush cycle.
const WindowSize = 60 * time.Second

// TenantID is an opaque identifier for a Blazil Cloud tenant.
//
// Using a named type instead of bare string prevents accidentally passing
// unrelated string values (e.g. a key hash, a user email) to billing-critical
// hot-path functions. The compiler rejects any implicit string assignment.
type TenantID string

// String implements fmt.Stringer so TenantID prints cleanly in logs.
func (t TenantID) String() string { return string(t) }

// Recorder tracks per-tenant transaction counts in memory.
// All methods are goroutine-safe.
type Recorder interface {
	// Record increments the tenant's counter by delta.
	// delta must be > 0.
	Record(tenantID TenantID, delta int64)

	// Snapshot atomically drains all non-zero counters and returns the deltas
	// accumulated since the last Snapshot call.
	// Tenants with a zero count are omitted from the returned map.
	Snapshot() map[TenantID]int64
}

// shardCount controls the number of lock stripes.
// Must be a power of two so that shardFor's bitmask works correctly.
const shardCount = 64

type tenantCounter struct {
	v atomic.Int64
}

type shard struct {
	mu       sync.RWMutex
	counters map[TenantID]*tenantCounter
}

// atomicRecorder shards tenants across shardCount mutexes to reduce lock
// contention at high request rates (200K+ TPS, thousands of tenants).
type atomicRecorder struct {
	shards [shardCount]shard
}

// NewRecorder returns a production-ready atomic Recorder.
func NewRecorder() Recorder {
	r := &atomicRecorder{}
	for i := range r.shards {
		r.shards[i].counters = make(map[TenantID]*tenantCounter)
	}
	return r
}

// shardFor maps a tenantID to a shard index using FNV-1a hashing.
func shardFor(tenantID TenantID) int {
	var h uint32 = 2166136261
	for i := 0; i < len(tenantID); i++ {
		h ^= uint32(tenantID[i])
		h *= 16777619
	}
	return int(h) & (shardCount - 1)
}

func (r *atomicRecorder) Record(tenantID TenantID, delta int64) {
	s := &r.shards[shardFor(tenantID)]

	// Fast path: counter already allocated.
	s.mu.RLock()
	c, ok := s.counters[tenantID]
	s.mu.RUnlock()
	if ok {
		c.v.Add(delta)
		return
	}

	// Slow path: allocate a new counter under write lock.
	s.mu.Lock()
	if c, ok = s.counters[tenantID]; !ok {
		c = &tenantCounter{}
		s.counters[tenantID] = c
	}
	s.mu.Unlock()
	c.v.Add(delta)
}

func (r *atomicRecorder) Snapshot() map[TenantID]int64 {
	result := make(map[TenantID]int64, 64)
	for i := range r.shards {
		s := &r.shards[i]
		s.mu.RLock()
		tenants := make([]TenantID, 0, len(s.counters))
		counters := make([]*tenantCounter, 0, len(s.counters))
		for t, c := range s.counters {
			tenants = append(tenants, t)
			counters = append(counters, c)
		}
		s.mu.RUnlock()
		for j, t := range tenants {
			if v := counters[j].v.Swap(0); v != 0 {
				result[t] = v
			}
		}
	}
	return result
}

// Package ratelimit provides per-tenant token bucket rate limiting.
//
// Each tenant has its own rate.Limiter configured with the tenant's
// rate_limit_rps and rate_limit_burst values from the tenant store.
// Limiters are lazily created and cached in a sync.Map.
//
// Design decisions:
//   - sync.Map is used instead of sync.RWMutex+map because the read path
//     (Allow) is overwhelmingly dominant and sync.Map's read fast-path
//     avoids any lock acquisition.
//   - golang.org/x/time/rate is the standard token-bucket implementation
//     in the Go ecosystem — production-tested, no external deps.
//   - The limiter for a tenant is re-created when its configuration changes
//     (rps or burst differ from the cached limiter). The old limiter is simply
//     replaced; in-flight Allow() calls on the old limiter drain naturally.
package ratelimit

import (
	"sync"

	"golang.org/x/time/rate"
)

// cachedLimiter wraps a rate.Limiter together with the rps/burst it was
// configured with, so we can detect configuration drift.
type cachedLimiter struct {
	limiter *rate.Limiter
	rps     int
	burst   int
}

// Limiter manages per-tenant rate.Limiters.
type Limiter struct {
	mu     sync.Mutex
	limits sync.Map // key: tenantID (string) → *cachedLimiter
}

// NewLimiter creates an empty Limiter.
func NewLimiter() *Limiter {
	return &Limiter{}
}

// Allow returns true if the tenant is within their rate limit budget.
//
// rps is the sustained rate (requests per second); burst is the maximum
// number of requests that can be served instantaneously before throttling
// begins. Both values come from the Tenant record returned by LookupAPIKey.
//
// When a tenant's rps or burst changes (e.g. after a plan upgrade), the cached
// limiter is transparently replaced with a new one seeded at full capacity.
func (l *Limiter) Allow(tenantID string, rps, burst int) bool {
	if v, ok := l.limits.Load(tenantID); ok {
		cl := v.(*cachedLimiter)
		if cl.rps == rps && cl.burst == burst {
			return cl.limiter.Allow()
		}
	}

	// Slow path: create or update the limiter under a per-key mutex to avoid
	// duplicate allocations under concurrent requests.
	l.mu.Lock()
	defer l.mu.Unlock()

	// Double-check after acquiring the lock.
	if v, ok := l.limits.Load(tenantID); ok {
		cl := v.(*cachedLimiter)
		if cl.rps == rps && cl.burst == burst {
			return cl.limiter.Allow()
		}
	}

	cl := &cachedLimiter{
		limiter: rate.NewLimiter(rate.Limit(rps), burst),
		rps:     rps,
		burst:   burst,
	}
	l.limits.Store(tenantID, cl)
	return cl.limiter.Allow()
}

// Delete removes the cached limiter for a tenant.
// Called when a tenant is deleted or suspended to free memory.
func (l *Limiter) Delete(tenantID string) {
	l.limits.Delete(tenantID)
}

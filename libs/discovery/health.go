package discovery

import (
	"context"
	"net"
	"time"

	"github.com/blazil/sharding"
)

// HealthChecker probes registered service endpoints at a fixed interval and
// updates a sharding.NodeRing to reflect the current health of each node.
// Only the engine TCP port is probed; a successful dial indicates the whole
// node is reachable.
//
// Default interval: 10 s. Default timeout: 2 s.
type HealthChecker struct {
	registry ServiceRegistry
	ring     *sharding.NodeRing
	interval time.Duration
	timeout  time.Duration
}

// HealthCheckerOption is a functional option for NewHealthChecker.
type HealthCheckerOption func(*HealthChecker)

// WithInterval sets the health-check polling interval.
func WithInterval(d time.Duration) HealthCheckerOption {
	return func(h *HealthChecker) { h.interval = d }
}

// WithTimeout sets the per-probe TCP-dial timeout.
func WithTimeout(d time.Duration) HealthCheckerOption {
	return func(h *HealthChecker) { h.timeout = d }
}

// NewHealthChecker constructs a HealthChecker that discovers engine endpoints
// from registry and reflects their health state into ring.
func NewHealthChecker(
	registry ServiceRegistry,
	ring *sharding.NodeRing,
	opts ...HealthCheckerOption,
) *HealthChecker {
	hc := &HealthChecker{
		registry: registry,
		ring:     ring,
		interval: 10 * time.Second,
		timeout:  2 * time.Second,
	}
	for _, o := range opts {
		o(hc)
	}
	return hc
}

// Start launches a background goroutine that polls health at hc.interval.
// The goroutine exits when ctx is cancelled.
func (h *HealthChecker) Start(ctx context.Context) {
	go func() {
		ticker := time.NewTicker(h.interval)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				h.checkAll(ctx)
			}
		}
	}()
}

// checkAll discovers engine endpoints and probes each one via TCP dial.
// Results are written to the NodeRing immediately.
func (h *HealthChecker) checkAll(ctx context.Context) {
	endpoints, err := h.registry.Discover(ctx, "engine")
	if err != nil {
		return
	}
	now := time.Now().UTC()
	for _, ep := range endpoints {
		status := sharding.NodeStatusDown
		if h.probe(ep.Address) {
			status = sharding.NodeStatusHealthy
		}
		h.ring.UpdateStatus(ep.NodeID, status, now)
	}
}

// probe attempts a TCP connection to address within hc.timeout.
// Returns true if the connection succeeds.
func (h *HealthChecker) probe(address string) bool {
	conn, err := net.DialTimeout("tcp", address, h.timeout)
	if err != nil {
		return false
	}
	_ = conn.Close()
	return true
}

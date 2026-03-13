package discovery

import (
	"context"
	"fmt"
	"sync"
)

// MockRegistry is a thread-safe in-memory ServiceRegistry for use in tests.
// It supports all four interface methods and tracks Watch call activity.
type MockRegistry struct {
	mu       sync.Mutex
	services map[string][]NodeEndpoint
	// WatchCalled is incremented each time Watch is invoked.
	WatchCalled int
	// RegisterCalled is incremented each time Register is invoked.
	RegisterCalled int
	// DiscoverErr, if set, is returned by Discover instead of the stored endpoints.
	DiscoverErr error
}

// NewMockRegistry constructs an empty MockRegistry.
func NewMockRegistry() *MockRegistry {
	return &MockRegistry{
		services: make(map[string][]NodeEndpoint),
	}
}

// Register adds an endpoint for node.Service and increments RegisterCalled.
func (m *MockRegistry) Register(_ context.Context, node NodeRegistration) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.RegisterCalled++
	ep := NodeEndpoint{
		NodeID:  node.NodeID,
		Address: node.Address,
		ShardID: node.ShardID,
		Healthy: true,
	}
	m.services[node.Service] = append(m.services[node.Service], ep)
	return nil
}

// Deregister removes all endpoints for nodeID from every service.
func (m *MockRegistry) Deregister(_ context.Context, nodeID string) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	for svc, eps := range m.services {
		var kept []NodeEndpoint
		for _, ep := range eps {
			if ep.NodeID != nodeID {
				kept = append(kept, ep)
			}
		}
		m.services[svc] = kept
	}
	return nil
}

// Discover returns the stored endpoints for service, or DiscoverErr if set.
func (m *MockRegistry) Discover(_ context.Context, service string) ([]NodeEndpoint, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.DiscoverErr != nil {
		return nil, m.DiscoverErr
	}
	eps := m.services[service]
	if len(eps) == 0 {
		return nil, fmt.Errorf("mock: no endpoints for service %q", service)
	}
	out := make([]NodeEndpoint, len(eps))
	copy(out, eps)
	return out, nil
}

// Watch sends the current endpoint list to ch once, then blocks until the
// context is cancelled. WatchCalled is incremented on each invocation.
func (m *MockRegistry) Watch(ctx context.Context, service string, ch chan<- []NodeEndpoint) error {
	m.mu.Lock()
	m.WatchCalled++
	m.mu.Unlock()

	eps, err := m.Discover(ctx, service)
	if err != nil {
		return err
	}
	ch <- eps
	<-ctx.Done()
	return nil
}

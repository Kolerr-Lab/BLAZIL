package discovery_test

import (
	"context"
	"net"
	"testing"
	"time"

	"github.com/blazil/discovery"
	"github.com/blazil/sharding"
)

// ── StaticRegistry tests ──────────────────────────────────────────────────────

// TestStaticRegistry_Discover verifies that a StaticRegistry populated via
// Register returns the correct endpoints for a given service name.
func TestStaticRegistry_Discover(t *testing.T) {
	t.Setenv("BLAZIL_NODES", "node-1:10.0.0.1:7878,node-2:10.0.0.2:7878,node-3:10.0.0.3:7878")

	reg, err := discovery.NewStaticRegistry()
	if err != nil {
		t.Fatalf("NewStaticRegistry: %v", err)
	}

	ctx := context.Background()
	for i, n := range []struct{ id, addr string }{
		{"node-1", "10.0.0.1:50051"},
		{"node-2", "10.0.0.2:50051"},
		{"node-3", "10.0.0.3:50051"},
	} {
		if err := reg.Register(ctx, discovery.NodeRegistration{
			NodeID:  n.id,
			Service: "payments",
			Address: n.addr,
			ShardID: i,
		}); err != nil {
			t.Fatalf("Register node-%d: %v", i+1, err)
		}
	}

	eps, err := reg.Discover(ctx, "payments")
	if err != nil {
		t.Fatalf("Discover: %v", err)
	}
	if len(eps) != 3 {
		t.Fatalf("expected 3 endpoints, got %d", len(eps))
	}
	for _, ep := range eps {
		if !ep.Healthy {
			t.Errorf("endpoint %s should be Healthy after Register", ep.NodeID)
		}
	}
}

// TestStaticRegistry_EmptyNodes_Error verifies that NewStaticRegistry returns
// an error when BLAZIL_NODES is not set.
func TestStaticRegistry_EmptyNodes_Error(t *testing.T) {
	t.Setenv("BLAZIL_NODES", "")

	_, err := discovery.NewStaticRegistry()
	if err == nil {
		t.Fatal("expected error for empty BLAZIL_NODES, got nil")
	}
}

// ── HealthChecker tests ───────────────────────────────────────────────────────

// TestHealthChecker_MarksHealthy verifies that the HealthChecker marks a node
// as Healthy when its TCP address is reachable.
func TestHealthChecker_MarksHealthy(t *testing.T) {
	// Start a real TCP listener to simulate a healthy engine.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer ln.Close()
	addr := ln.Addr().String()

	// Accept connections in the background so probes don't hang.
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			conn.Close()
		}
	}()

	mock := discovery.NewMockRegistry()
	ctx := context.Background()
	_ = mock.Register(ctx, discovery.NodeRegistration{
		NodeID:  "n0",
		Service: "engine",
		Address: addr,
		ShardID: 0,
	})

	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", Address: addr, ShardID: 0, Status: sharding.NodeStatusDown})

	hc := discovery.NewHealthChecker(mock, ring,
		discovery.WithInterval(20*time.Millisecond),
		discovery.WithTimeout(500*time.Millisecond),
	)
	hc.Start(ctx)

	// Wait long enough for at least one health check cycle.
	time.Sleep(80 * time.Millisecond)

	node, err := ring.Get(0)
	if err != nil {
		t.Fatalf("ring.Get(0): %v", err)
	}
	if node.Status != sharding.NodeStatusHealthy {
		t.Errorf("expected NodeStatusHealthy, got %s", node.Status)
	}
}

// TestHealthChecker_MarksUnhealthy verifies that the HealthChecker marks a
// node as Down when its TCP address is unreachable.
func TestHealthChecker_MarksUnhealthy(t *testing.T) {
	// Use a port that is very unlikely to be in use.
	addr := "127.0.0.1:19978"

	mock := discovery.NewMockRegistry()
	ctx := context.Background()
	_ = mock.Register(ctx, discovery.NodeRegistration{
		NodeID:  "n0",
		Service: "engine",
		Address: addr,
		ShardID: 0,
	})

	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{
		ID:      "n0",
		Address: addr,
		ShardID: 0,
		Status:  sharding.NodeStatusHealthy, // start Healthy, expect Down
	})

	hc := discovery.NewHealthChecker(mock, ring,
		discovery.WithInterval(20*time.Millisecond),
		discovery.WithTimeout(100*time.Millisecond),
	)
	hc.Start(ctx)

	time.Sleep(80 * time.Millisecond)

	node, err := ring.Get(0)
	if err == nil && node.Status == sharding.NodeStatusHealthy {
		t.Error("expected node to be marked Down after failed TCP probe")
	}
}

// TestHealthChecker_UpdatesNodeRing verifies that the HealthChecker correctly
// sets one node Healthy and another Down in the same ring.
func TestHealthChecker_UpdatesNodeRing(t *testing.T) {
	// Node A — healthy: bind a real listener.
	lnA, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen A: %v", err)
	}
	defer lnA.Close()
	addrA := lnA.Addr().String()
	go func() {
		for {
			c, err := lnA.Accept()
			if err != nil {
				return
			}
			c.Close()
		}
	}()

	// Node B — unhealthy: nothing listening on this port.
	addrB := "127.0.0.1:19979"

	mock := discovery.NewMockRegistry()
	ctx := context.Background()
	_ = mock.Register(ctx, discovery.NodeRegistration{NodeID: "nA", Service: "engine", Address: addrA, ShardID: 0})
	_ = mock.Register(ctx, discovery.NodeRegistration{NodeID: "nB", Service: "engine", Address: addrB, ShardID: 1})

	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "nA", Address: addrA, ShardID: 0, Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "nB", Address: addrB, ShardID: 1, Status: sharding.NodeStatusHealthy})

	hc := discovery.NewHealthChecker(mock, ring,
		discovery.WithInterval(20*time.Millisecond),
		discovery.WithTimeout(100*time.Millisecond),
	)
	hc.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	nodeA, err := ring.Get(0)
	if err != nil {
		t.Fatalf("ring.Get(0): %v", err)
	}
	if nodeA.Status != sharding.NodeStatusHealthy {
		t.Errorf("expected nA Healthy, got %s", nodeA.Status)
	}

	// Check that nB is no longer Healthy by inspecting All() nodes.
	allNodes := ring.All()
	var foundB bool
	for _, n := range allNodes {
		if n.ID == "nB" {
			foundB = true
			if n.Status == sharding.NodeStatusHealthy {
				t.Error("expected nB Down or Degraded, got Healthy")
			}
		}
	}
	if !foundB {
		t.Error("nB not found in ring")
	}
}

// ── MockRegistry tests ────────────────────────────────────────────────────────

// TestMockRegistry_Watch verifies that Watch sends the initial endpoint list
// to the channel and then blocks until the context is cancelled.
func TestMockRegistry_Watch(t *testing.T) {
	mock := discovery.NewMockRegistry()
	ctx := context.Background()

	_ = mock.Register(ctx, discovery.NodeRegistration{
		NodeID:  "n0",
		Service: "payments",
		Address: "10.0.0.1:50051",
		ShardID: 0,
	})

	ch := make(chan []discovery.NodeEndpoint, 1)
	watchCtx, cancel := context.WithCancel(ctx)

	go func() {
		_ = mock.Watch(watchCtx, "payments", ch)
	}()

	select {
	case eps := <-ch:
		if len(eps) != 1 {
			t.Errorf("expected 1 endpoint, got %d", len(eps))
		}
		if eps[0].NodeID != "n0" {
			t.Errorf("expected nodeID n0, got %s", eps[0].NodeID)
		}
	case <-time.After(500 * time.Millisecond):
		t.Fatal("timed out waiting for Watch to send initial endpoint list")
	}

	cancel() // should unblock Watch goroutine

	if mock.WatchCalled != 1 {
		t.Errorf("expected WatchCalled=1, got %d", mock.WatchCalled)
	}
}

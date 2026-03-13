package sharding_test

import (
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/blazil/sharding"
)

// ── JumpHash tests ────────────────────────────────────────────────────────────

// TestJumpHash_Deterministic verifies that the same key always produces the
// same shard ID regardless of how many times it is called.
func TestJumpHash_Deterministic(t *testing.T) {
	for i := 0; i < 500; i++ {
		key := uint64(i) * 2862933555777941757
		a := sharding.JumpHash(key, 3)
		b := sharding.JumpHash(key, 3)
		if a != b {
			t.Errorf("JumpHash not deterministic: key=%d got %d then %d", key, a, b)
		}
		if a < 0 || a >= 3 {
			t.Errorf("JumpHash(%d, 3) = %d, want [0,3)", key, a)
		}
	}
}

// TestJumpHash_Distribution verifies that 1 000 mixed keys are spread across
// 3 shards within 10% of the expected 333 per shard (i.e. 290–380 each).
func TestJumpHash_Distribution(t *testing.T) {
	counts := make([]int, 3)
	for i := 0; i < 1000; i++ {
		// Mix the sequential key with a multiplier to get representative input.
		key := uint64(i)*2862933555777941757 + 1
		shard := sharding.JumpHash(key, 3)
		counts[shard]++
	}
	for i, c := range counts {
		if c < 290 || c > 380 {
			t.Errorf("shard %d has %d accounts, want 290–380 (±~14%% of 333)", i, c)
		}
	}
}

// TestJumpHash_Stability verifies that adding a 4th shard to a 3-shard cluster
// remaps fewer than 30% of keys — close to the theoretical optimum of 1/(n+1)
// = 25%.
func TestJumpHash_Stability(t *testing.T) {
	const total = 10_000
	remapped := 0
	for i := 0; i < total; i++ {
		key := uint64(i) * 2862933555777941757
		if sharding.JumpHash(key, 3) != sharding.JumpHash(key, 4) {
			remapped++
		}
	}
	pct := float64(remapped) / float64(total)
	if pct >= 0.30 {
		t.Errorf("too many keys remapped when adding shard: %.1f%% (want <30%%)", pct*100)
	}
}

// ── NodeRing tests ────────────────────────────────────────────────────────────

// TestNodeRing_AddRemove verifies that nodes can be added and removed and that
// the ring size tracks correctly.
func TestNodeRing_AddRemove(t *testing.T) {
	ring := &sharding.NodeRing{}

	nodes := []sharding.NodeInfo{
		{ID: "n0", ShardID: 0, Address: "host:7878", Status: sharding.NodeStatusHealthy},
		{ID: "n1", ShardID: 1, Address: "host:7879", Status: sharding.NodeStatusHealthy},
		{ID: "n2", ShardID: 2, Address: "host:7880", Status: sharding.NodeStatusHealthy},
	}
	for _, n := range nodes {
		if err := ring.Add(n); err != nil {
			t.Fatalf("Add(%s) error: %v", n.ID, err)
		}
	}
	if ring.Size() != 3 {
		t.Fatalf("expected size 3, got %d", ring.Size())
	}

	// Duplicate add must fail.
	if err := ring.Add(nodes[0]); err == nil {
		t.Error("expected error adding duplicate node")
	}

	if err := ring.Remove("n1"); err != nil {
		t.Fatalf("Remove n1 error: %v", err)
	}
	if ring.Size() != 2 {
		t.Fatalf("expected size 2 after remove, got %d", ring.Size())
	}

	// Remove non-existent must fail.
	if err := ring.Remove("n1"); err == nil {
		t.Error("expected error removing non-existent node")
	}
}

// TestNodeRing_GetHealthy verifies that Get skips nodes whose status is Down.
func TestNodeRing_GetHealthy(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Status: sharding.NodeStatusHealthy})

	// Down node must not be returned.
	if _, err := ring.Get(0); err == nil {
		t.Error("expected error for Down node on shard 0")
	}

	// Healthy node must be returned.
	node, err := ring.Get(1)
	if err != nil {
		t.Fatalf("Get(1) error: %v", err)
	}
	if node.ID != "n1" {
		t.Errorf("expected n1, got %s", node.ID)
	}
}

// ── ShardRouter tests ─────────────────────────────────────────────────────────

// TestShardRouter_RouteByAccount verifies that the same accountID always
// resolves to the same node.
func TestShardRouter_RouteByAccount(t *testing.T) {
	ring := threeNodeRing(t)
	router := sharding.NewShardRouter(ring, 3)

	const accountID = uint64(987654321)
	first, err := router.RouteByAccount(accountID)
	if err != nil {
		t.Fatalf("RouteByAccount error: %v", err)
	}
	for i := 0; i < 100; i++ {
		node, err := router.RouteByAccount(accountID)
		if err != nil {
			t.Fatalf("RouteByAccount[%d] error: %v", i, err)
		}
		if node.ID != first.ID {
			t.Errorf("routing not deterministic: got %s, want %s", node.ID, first.ID)
		}
	}
}

// TestShardRouter_IsCrossShard_True verifies that two accounts on different
// shards are correctly identified as a cross-shard pair.
func TestShardRouter_IsCrossShard_True(t *testing.T) {
	ring := threeNodeRing(t)
	router := sharding.NewShardRouter(ring, 3)

	// Find two keys that land on different shards.
	var a, b uint64
	found := false
	for i := uint64(0); i < 10_000; i++ {
		ka := i * 2862933555777941757
		for j := uint64(i + 1); j < i+100; j++ {
			kb := j * 2862933555777941757
			if sharding.JumpHash(ka, 3) != sharding.JumpHash(kb, 3) {
				a, b = ka, kb
				found = true
				break
			}
		}
		if found {
			break
		}
	}
	if !found {
		t.Fatal("could not find two accounts on different shards")
	}
	if !router.IsCrossShard(a, b) {
		t.Errorf("IsCrossShard(%d, %d) = false, want true", a, b)
	}
}

// TestShardRouter_IsCrossShard_False verifies that the same account ID reports
// no cross-shard requirement.
func TestShardRouter_IsCrossShard_False(t *testing.T) {
	ring := threeNodeRing(t)
	router := sharding.NewShardRouter(ring, 3)

	const id = uint64(111222333)
	if router.IsCrossShard(id, id) {
		t.Error("IsCrossShard(x, x) = true, want false")
	}
}

// TestShardRouter_NoHealthyNodes_Error verifies that RouteByAccount returns an
// error when all nodes are Down.
func TestShardRouter_NoHealthyNodes_Error(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "n2", ShardID: 2, Status: sharding.NodeStatusDown})
	router := sharding.NewShardRouter(ring, 3)

	if _, err := router.RouteByAccount(42); err == nil {
		t.Error("expected error when all nodes are Down")
	}
}

// ── LoadBalancer tests ────────────────────────────────────────────────────────

// TestLoadBalancer_GetConnection verifies that GetConnection returns a non-nil
// gRPC connection for a valid account in a healthy ring.
func TestLoadBalancer_GetConnection(t *testing.T) {
	ring := threeNodeRing(t)
	router := sharding.NewShardRouter(ring, 3)
	balancer := sharding.NewShardAwareLoadBalancer(router, ring)
	defer balancer.Close() //nolint:errcheck

	// JumpHash(0, 3) lands on shard 0, which is healthy.
	conn, err := balancer.GetConnection(0)
	if err != nil {
		t.Fatalf("GetConnection error: %v", err)
	}
	if conn == nil {
		t.Error("expected non-nil gRPC connection")
	}
}

// TestLoadBalancer_NodeDown_Failover verifies that when the primary shard
// node is Down the balancer falls back to another healthy node.
func TestLoadBalancer_NodeDown_Failover(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Address: "localhost:50190", Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Address: "localhost:50191", Status: sharding.NodeStatusHealthy})

	router := sharding.NewShardRouter(ring, 2)
	balancer := sharding.NewShardAwareLoadBalancer(router, ring)
	defer balancer.Close() //nolint:errcheck

	// Find an accountID that hashes to shard 0 (the Down node).
	var accountID uint64
	for i := uint64(1); i < 1_000; i++ {
		if sharding.JumpHash(i, 2) == 0 {
			accountID = i
			break
		}
	}

	conn, err := balancer.GetConnection(accountID)
	if err != nil {
		t.Fatalf("expected failover to healthy node, got error: %v", err)
	}
	if conn == nil {
		t.Error("expected non-nil connection after failover")
	}
}

// ── Rebalancer tests ──────────────────────────────────────────────────────────

// TestRebalancer_ShouldRebalance_NodeDown verifies that ShouldRebalance returns
// true when at least one node is Down.
func TestRebalancer_ShouldRebalance_NodeDown(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Status: sharding.NodeStatusHealthy})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Status: sharding.NodeStatusDown})

	r := &sharding.SimpleRebalancer{}
	if !r.ShouldRebalance(ring) {
		t.Error("ShouldRebalance = false, want true when a node is Down")
	}
}

// TestRebalancer_Plan_MovesShards verifies that Plan produces a move for every
// Down node, sourced from the Down node and destined for a healthy node.
func TestRebalancer_Plan_MovesShards(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Status: sharding.NodeStatusDown})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Status: sharding.NodeStatusHealthy})
	_ = ring.Add(sharding.NodeInfo{ID: "n2", ShardID: 2, Status: sharding.NodeStatusHealthy})

	r := &sharding.SimpleRebalancer{}
	moves, err := r.Plan(ring)
	if err != nil {
		t.Fatalf("Plan error: %v", err)
	}
	if len(moves) == 0 {
		t.Fatal("expected at least one rebalance move")
	}
	// The Down node n0 must appear as a source.
	found := false
	for _, m := range moves {
		if m.FromNode == "n0" {
			found = true
			if m.ToNode != "n1" && m.ToNode != "n2" {
				t.Errorf("move targets non-healthy node %s", m.ToNode)
			}
		}
	}
	if !found {
		t.Error("expected a move sourced from Down node n0")
	}
}

// ── MockShardRouter tests ─────────────────────────────────────────────────────

// TestMockShardRouter_TracksCalls verifies that RouteByAccount and IsCrossShard
// call counts are correctly tracked.
func TestMockShardRouter_TracksCalls(t *testing.T) {
	mock := sharding.NewMockShardRouter()

	_, _ = mock.RouteByAccount(1)
	_, _ = mock.RouteByAccount(2)
	mock.IsCrossShard(1, 2)

	if mock.RouteByAccountCallCount() != 2 {
		t.Errorf("RouteByAccountCallCount = %d, want 2", mock.RouteByAccountCallCount())
	}
	if mock.IsCrossShardCallCount() != 1 {
		t.Errorf("IsCrossShardCallCount = %d, want 1", mock.IsCrossShardCallCount())
	}
}

// TestMockShardRouter_ErrorInjection verifies that SetShardError causes
// RouteByAccount to return the configured error for the matching shard.
func TestMockShardRouter_ErrorInjection(t *testing.T) {
	mock := sharding.NewMockShardRouter()
	// Find which shard accountID 42 maps to, then inject an error for it.
	shard := sharding.JumpHash(42, 3)
	mock.SetShardError(shard, errors.New("injected error"))

	_, err := mock.RouteByAccount(42)
	if err == nil {
		t.Error("expected injected error, got nil")
	}

	// Clearing the error restores normal routing.
	mock.SetShardError(shard, nil)
	_, err = mock.RouteByAccount(42)
	if err != nil {
		t.Errorf("expected no error after clearing, got: %v", err)
	}
}

// ── NodeRing.UpdateStatus test ────────────────────────────────────────────────

// TestNodeRing_UpdateStatus verifies that UpdateStatus changes a node's health
// and LastSeen fields without affecting other nodes.
func TestNodeRing_UpdateStatus(t *testing.T) {
	ring := &sharding.NodeRing{}
	_ = ring.Add(sharding.NodeInfo{ID: "n0", ShardID: 0, Status: sharding.NodeStatusHealthy})
	_ = ring.Add(sharding.NodeInfo{ID: "n1", ShardID: 1, Status: sharding.NodeStatusHealthy})

	now := time.Now().UTC()
	ring.UpdateStatus("n0", sharding.NodeStatusDegraded, now)

	// n0 should now be Degraded (Get returns it unless Down).
	node, err := ring.Get(0)
	if err != nil {
		t.Fatalf("Get(0) after UpdateStatus error: %v", err)
	}
	if node.Status != sharding.NodeStatusDegraded {
		t.Errorf("expected Degraded, got %v", node.Status)
	}

	// n1 must be unaffected.
	n1, err := ring.Get(1)
	if err != nil {
		t.Fatalf("Get(1) error: %v", err)
	}
	if n1.Status != sharding.NodeStatusHealthy {
		t.Errorf("n1 status changed unexpectedly: %v", n1.Status)
	}
}

// ── helpers ───────────────────────────────────────────────────────────────────

// threeNodeRing builds a healthy 3-node, 3-shard ring for use in tests.
func threeNodeRing(t *testing.T) *sharding.NodeRing {
	t.Helper()
	ring := &sharding.NodeRing{}
	for i := 0; i < 3; i++ {
		err := ring.Add(sharding.NodeInfo{
			ID:      fmt.Sprintf("n%d", i),
			ShardID: i,
			Address: fmt.Sprintf("127.0.0.1:%d", 50200+i),
			Status:  sharding.NodeStatusHealthy,
		})
		if err != nil {
			t.Fatalf("Add(n%d): %v", i, err)
		}
	}
	return ring
}

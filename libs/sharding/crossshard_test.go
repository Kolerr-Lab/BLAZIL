package sharding_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/sharding"
)

// mockTransferClient is a test double for NodeTransferClient that records
// whether Void was called and can be configured to fail at any step.
type mockTransferClient struct {
	pendingErr error
	commitErr  error
	voidCalled bool
}

func (m *mockTransferClient) SubmitPending(_ context.Context, _ sharding.CrossShardRequest) (string, error) {
	if m.pendingErr != nil {
		return "", m.pendingErr
	}
	return "transfer-abc123", nil
}

func (m *mockTransferClient) Commit(_ context.Context, _ string) error {
	return m.commitErr
}

func (m *mockTransferClient) Void(_ context.Context, _ string) error {
	m.voidCalled = true
	return nil
}

// buildCoordinator wires a CrossShardCoordinatorImpl using a 3-shard NodeRing
// and the supplied address→mockTransferClient map.
func buildCoordinator(mocks map[string]*mockTransferClient) *sharding.CrossShardCoordinatorImpl {
	ring := &sharding.NodeRing{}
	nodes := []sharding.NodeInfo{
		{ID: "n0", Address: "node0:7878", ShardID: 0, Status: sharding.NodeStatusHealthy},
		{ID: "n1", Address: "node1:7878", ShardID: 1, Status: sharding.NodeStatusHealthy},
		{ID: "n2", Address: "node2:7878", ShardID: 2, Status: sharding.NodeStatusHealthy},
	}
	for _, n := range nodes {
		_ = ring.Add(n)
	}
	router := sharding.NewShardRouter(ring, 3)
	balancer := sharding.NewShardAwareLoadBalancer(router, ring)
	factory := func(addr string) sharding.NodeTransferClient {
		if m, ok := mocks[addr]; ok {
			return m
		}
		return &mockTransferClient{}
	}
	return sharding.NewCrossShardCoordinator(router, balancer, factory)
}

// findCrossShardPair returns two account IDs that JumpHash to different shards
// in a 3-shard ring.
func findCrossShardPair() (from, to uint64) {
	from = 1
	fromShard := sharding.JumpHash(from, 3)
	for to = 2; sharding.JumpHash(to, 3) == fromShard; to++ {
	}
	return from, to
}

// findSameShardPair returns two account IDs that JumpHash to the same shard.
func findSameShardPair() (a, b uint64) {
	for a = 0; a < 1000; a++ {
		for b = a + 1; b < 1000; b++ {
			if sharding.JumpHash(a, 3) == sharding.JumpHash(b, 3) {
				return a, b
			}
		}
	}
	panic("could not find same-shard pair in [0, 1000)")
}

// TestCrossShardCoordinator_SameShard_Error verifies that Execute returns an
// error when both accounts resolve to the same shard, directing callers to use
// the normal single-shard transfer path instead.
func TestCrossShardCoordinator_SameShard_Error(t *testing.T) {
	mocks := map[string]*mockTransferClient{
		"node0:7878": {},
		"node1:7878": {},
		"node2:7878": {},
	}
	coord := buildCoordinator(mocks)

	a, b := findSameShardPair()

	err := coord.Execute(context.Background(), sharding.CrossShardRequest{
		FromAccountID:  a,
		ToAccountID:    b,
		Amount:         1000,
		Currency:       "USD",
		IdempotencyKey: "same-shard-test",
	})
	if err == nil {
		t.Fatalf("expected error for same-shard pair (%d, %d), got nil", a, b)
	}
}

// TestCrossShardCoordinator_Execute_Success verifies that a cross-shard
// transfer where pending and commit both succeed returns nil.
func TestCrossShardCoordinator_Execute_Success(t *testing.T) {
	mocks := map[string]*mockTransferClient{
		"node0:7878": {},
		"node1:7878": {},
		"node2:7878": {},
	}
	coord := buildCoordinator(mocks)

	from, to := findCrossShardPair()

	err := coord.Execute(context.Background(), sharding.CrossShardRequest{
		FromAccountID:  from,
		ToAccountID:    to,
		Amount:         5000,
		Currency:       "USD",
		IdempotencyKey: "success-xshard-1",
	})
	if err != nil {
		t.Fatalf("expected nil error, got: %v", err)
	}
}

// TestCrossShardCoordinator_CommitFail_Voids verifies that when the commit step
// fails, the coordinator calls Void on the source node and returns an error.
func TestCrossShardCoordinator_CommitFail_Voids(t *testing.T) {
	from, to := findCrossShardPair()
	fromShard := sharding.JumpHash(from, 3)
	toShard := sharding.JumpHash(to, 3)

	nodeAddrs := []string{"node0:7878", "node1:7878", "node2:7878"}
	sourceMock := &mockTransferClient{} // pending succeeds
	destMock := &mockTransferClient{commitErr: errors.New("dest engine unreachable")}
	thirdShard := 3 - fromShard - toShard // 0+1+2 = 3, so third = 3 - from - to

	mocks := map[string]*mockTransferClient{
		nodeAddrs[fromShard]:  sourceMock,
		nodeAddrs[toShard]:    destMock,
		nodeAddrs[thirdShard]: {},
	}
	coord := buildCoordinator(mocks)

	err := coord.Execute(context.Background(), sharding.CrossShardRequest{
		FromAccountID:  from,
		ToAccountID:    to,
		Amount:         9999,
		Currency:       "USD",
		IdempotencyKey: "commit-fail-xshard-1",
	})
	if err == nil {
		t.Fatal("expected error when commit fails, got nil")
	}
	if !sourceMock.voidCalled {
		t.Error("expected Void to be called on source node after commit failure, but it was not")
	}
}

// TestCrossShardCoordinator_IdempotencyKey verifies that calling Execute twice
// with the same idempotency key returns the cached result without re-executing.
func TestCrossShardCoordinator_IdempotencyKey(t *testing.T) {
	mocks := map[string]*mockTransferClient{
		"node0:7878": {},
		"node1:7878": {},
		"node2:7878": {},
	}
	coord := buildCoordinator(mocks)

	from, to := findCrossShardPair()
	req := sharding.CrossShardRequest{
		FromAccountID:  from,
		ToAccountID:    to,
		Amount:         100,
		Currency:       "USD",
		IdempotencyKey: "idem-xshard-key-1",
	}

	// First call must succeed.
	if err := coord.Execute(context.Background(), req); err != nil {
		t.Fatalf("first call: unexpected error: %v", err)
	}

	// Poison all mocks so a real re-execution would fail.
	for _, m := range mocks {
		m.pendingErr = errors.New("SubmitPending must not be called again")
	}

	// Second call with the same key must return the cached nil (success).
	if err := coord.Execute(context.Background(), req); err != nil {
		t.Fatalf("second call (cached): expected nil, got: %v", err)
	}
}

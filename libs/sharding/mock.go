package sharding

import (
	"fmt"
	"sync"
)

// MockShardRouter is a test double for ShardRouter. It uses the real jump
// consistent hash for shard assignment and tracks call counts for all methods.
// Error injection is supported per-shard via SetShardError.
//
// Default configuration: 3 shards, 3 in-memory nodes.
type MockShardRouter struct {
	mu                  sync.Mutex
	NumShardsVal        int
	Nodes               []NodeInfo
	ErrorShards         map[int]error

	routeByAccountCalls int
	isCrossShardCalls   int
}

// NewMockShardRouter returns a MockShardRouter pre-configured with 3 nodes
// and 3 shards. All nodes are healthy.
func NewMockShardRouter() *MockShardRouter {
	return &MockShardRouter{
		NumShardsVal: 3,
		Nodes: []NodeInfo{
			{ID: "mock-node-0", Address: "mock:7878", ShardID: 0, Status: NodeStatusHealthy},
			{ID: "mock-node-1", Address: "mock:7879", ShardID: 1, Status: NodeStatusHealthy},
			{ID: "mock-node-2", Address: "mock:7880", ShardID: 2, Status: NodeStatusHealthy},
		},
		ErrorShards: make(map[int]error),
	}
}

// RouteByAccount implements ShardRouter. Increments the RouteByAccount call
// counter and returns an error if the resolved shard has one configured.
func (m *MockShardRouter) RouteByAccount(accountID uint64) (NodeInfo, error) {
	m.mu.Lock()
	m.routeByAccountCalls++
	m.mu.Unlock()

	shard := JumpHash(accountID, m.NumShardsVal)
	if err, ok := m.ErrorShards[shard]; ok {
		return NodeInfo{}, err
	}
	if shard >= len(m.Nodes) {
		return NodeInfo{}, fmt.Errorf("mock: shard %d out of range", shard)
	}
	return m.Nodes[shard], nil
}

// RouteByShardID implements ShardRouter.
func (m *MockShardRouter) RouteByShardID(shardID int) (NodeInfo, error) {
	if shardID < 0 || shardID >= len(m.Nodes) {
		return NodeInfo{}, fmt.Errorf("mock: shard %d out of range", shardID)
	}
	return m.Nodes[shardID], nil
}

// IsCrossShard implements ShardRouter. Increments the IsCrossShard call
// counter and computes the real jump hash result.
func (m *MockShardRouter) IsCrossShard(fromAccount, toAccount uint64) bool {
	m.mu.Lock()
	m.isCrossShardCalls++
	m.mu.Unlock()
	return JumpHash(fromAccount, m.NumShardsVal) != JumpHash(toAccount, m.NumShardsVal)
}

// NumShards implements ShardRouter.
func (m *MockShardRouter) NumShards() int {
	return m.NumShardsVal
}

// RouteByAccountCallCount returns the number of RouteByAccount calls received.
func (m *MockShardRouter) RouteByAccountCallCount() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.routeByAccountCalls
}

// IsCrossShardCallCount returns the number of IsCrossShard calls received.
func (m *MockShardRouter) IsCrossShardCallCount() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.isCrossShardCalls
}

// SetShardError configures err to be returned whenever RouteByAccount resolves
// to shardID. Pass nil to clear a previously configured error.
func (m *MockShardRouter) SetShardError(shardID int, err error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if err == nil {
		delete(m.ErrorShards, shardID)
	} else {
		m.ErrorShards[shardID] = err
	}
}

package sharding

import "fmt"

// ShardRouter routes requests to the correct shard node based on account IDs.
// Implementations must be safe for concurrent use.
type ShardRouter interface {
	// RouteByAccount returns the node responsible for the given accountID.
	// Returns an error if no healthy node owns the target shard.
	RouteByAccount(accountID uint64) (NodeInfo, error)
	// RouteByShardID returns the node that owns the given shard.
	// Returns an error if no healthy node owns that shard.
	RouteByShardID(shardID int) (NodeInfo, error)
	// IsCrossShard reports whether fromAccount and toAccount belong to
	// different shards. Single-account operations that return false require
	// no cross-node coordination.
	IsCrossShard(fromAccount, toAccount uint64) bool
	// NumShards returns the total number of shards in the cluster.
	NumShards() int
}

// ShardRouterImpl is the production implementation of ShardRouter. It uses
// jump consistent hash to achieve O(ln n) routing with minimal remapping.
type ShardRouterImpl struct {
	hasher ConsistentHasher
	ring   *NodeRing
	shards int
}

// NewShardRouter constructs a ShardRouterImpl backed by ring with shards total
// shard partitions. shards must be > 0.
func NewShardRouter(ring *NodeRing, shards int) *ShardRouterImpl {
	return &ShardRouterImpl{
		hasher: ConsistentHasher{},
		ring:   ring,
		shards: shards,
	}
}

// RouteByAccount implements ShardRouter.
func (r *ShardRouterImpl) RouteByAccount(accountID uint64) (NodeInfo, error) {
	shardID := r.hasher.ShardOf(accountID, r.shards)
	node, err := r.ring.Get(shardID)
	if err != nil {
		return NodeInfo{}, fmt.Errorf("sharding: route account %d (shard %d): %w", accountID, shardID, err)
	}
	return node, nil
}

// RouteByShardID implements ShardRouter.
func (r *ShardRouterImpl) RouteByShardID(shardID int) (NodeInfo, error) {
	node, err := r.ring.Get(shardID)
	if err != nil {
		return NodeInfo{}, fmt.Errorf("sharding: route shard %d: %w", shardID, err)
	}
	return node, nil
}

// IsCrossShard implements ShardRouter.
func (r *ShardRouterImpl) IsCrossShard(fromAccount, toAccount uint64) bool {
	fromShard := r.hasher.ShardOf(fromAccount, r.shards)
	toShard := r.hasher.ShardOf(toAccount, r.shards)
	return fromShard != toShard
}

// NumShards implements ShardRouter.
func (r *ShardRouterImpl) NumShards() int {
	return r.shards
}

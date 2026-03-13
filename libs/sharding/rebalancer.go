package sharding

import "fmt"

// RebalanceStrategy decides when and how to move shards between nodes.
type RebalanceStrategy interface {
	// ShouldRebalance reports whether the current ring topology warrants a
	// rebalance operation (e.g. one or more nodes are Down).
	ShouldRebalance(ring *NodeRing) bool
	// Plan returns the list of shard moves required to restore balance.
	// It does not execute any data migration; that is the infrastructure
	// layer's responsibility.
	Plan(ring *NodeRing) ([]RebalanceMove, error)
}

// RebalanceMove describes a single shard relocation from one node to another.
// Actual data migration is performed by the infrastructure layer; this struct
// is a plan only.
type RebalanceMove struct {
	// FromNode is the ID of the node currently owning the shard.
	FromNode string
	// ToNode is the ID of the node that should receive the shard.
	ToNode string
	// ShardID is the shard being relocated.
	ShardID int
	// AccountRange is the [min, max] uint64 account ID range covered by this
	// shard move. [0, ^uint64(0)] means the full key space.
	AccountRange [2]uint64
}

// SimpleRebalancer implements RebalanceStrategy with a straightforward policy:
// rebalance whenever any node is Down, and move all Down-node shards to healthy
// nodes using round-robin assignment.
type SimpleRebalancer struct{}

// ShouldRebalance returns true if any node in the ring has NodeStatusDown.
func (r *SimpleRebalancer) ShouldRebalance(ring *NodeRing) bool {
	for _, n := range ring.All() {
		if n.Status == NodeStatusDown {
			return true
		}
	}
	return false
}

// Plan builds a rebalance plan that moves every shard owned by a Down node to
// a healthy node using round-robin assignment. Returns an error if there are
// no healthy nodes to receive shards.
func (r *SimpleRebalancer) Plan(ring *NodeRing) ([]RebalanceMove, error) {
	healthy := ring.Healthy()
	if len(healthy) == 0 {
		return nil, fmt.Errorf("sharding: rebalance: no healthy nodes available to receive shards")
	}

	var moves []RebalanceMove
	healthyIdx := 0
	for _, node := range ring.All() {
		if node.Status != NodeStatusDown {
			continue
		}
		target := healthy[healthyIdx%len(healthy)]
		healthyIdx++
		moves = append(moves, RebalanceMove{
			FromNode:     node.ID,
			ToNode:       target.ID,
			ShardID:      node.ShardID,
			AccountRange: [2]uint64{0, ^uint64(0)},
		})
	}
	return moves, nil
}

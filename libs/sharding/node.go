package sharding

import (
	"fmt"
	"sync"
	"time"
)

// NodeStatus represents the health state of a shard node.
type NodeStatus int

const (
	// NodeStatusHealthy indicates the node is fully operational.
	NodeStatusHealthy NodeStatus = iota
	// NodeStatusDegraded indicates the node is reachable but performing poorly.
	NodeStatusDegraded
	// NodeStatusDown indicates the node is unreachable or failed.
	NodeStatusDown
)

// String returns a human-readable name for the node status.
func (s NodeStatus) String() string {
	switch s {
	case NodeStatusHealthy:
		return "healthy"
	case NodeStatusDegraded:
		return "degraded"
	case NodeStatusDown:
		return "down"
	default:
		return "unknown"
	}
}

// NodeInfo describes a single node in the shard ring.
type NodeInfo struct {
	// ID is a unique human-readable node identifier (e.g. "node-1").
	ID string
	// Address is the network address of the node (e.g. "10.0.0.1:7878").
	Address string
	// ShardID is the shard this node is responsible for.
	ShardID int
	// Status is the current health state of the node.
	Status NodeStatus
	// LastSeen is the UTC timestamp of the most recent health check.
	LastSeen time.Time
}

// NodeRing manages the ordered set of shard nodes. All methods are safe for
// concurrent use.
type NodeRing struct {
	nodes []NodeInfo
	mu    sync.RWMutex
}

// Add inserts node into the ring. Returns an error if a node with the same ID
// already exists.
func (r *NodeRing) Add(node NodeInfo) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, n := range r.nodes {
		if n.ID == node.ID {
			return fmt.Errorf("sharding: node %q already exists in ring", node.ID)
		}
	}
	r.nodes = append(r.nodes, node)
	return nil
}

// Remove deletes the node identified by nodeID from the ring. Returns an error
// if no such node exists.
func (r *NodeRing) Remove(nodeID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	for i, n := range r.nodes {
		if n.ID == nodeID {
			r.nodes = append(r.nodes[:i], r.nodes[i+1:]...)
			return nil
		}
	}
	return fmt.Errorf("sharding: node %q not found in ring", nodeID)
}

// Get returns the healthy node that owns shardID. Returns an error if no
// healthy node is assigned to that shard.
func (r *NodeRing) Get(shardID int) (NodeInfo, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	for _, n := range r.nodes {
		if n.ShardID == shardID && n.Status != NodeStatusDown {
			return n, nil
		}
	}
	return NodeInfo{}, fmt.Errorf("sharding: no healthy node for shard %d", shardID)
}

// Healthy returns a snapshot of all nodes whose status is not NodeStatusDown.
func (r *NodeRing) Healthy() []NodeInfo {
	r.mu.RLock()
	defer r.mu.RUnlock()
	var out []NodeInfo
	for _, n := range r.nodes {
		if n.Status != NodeStatusDown {
			out = append(out, n)
		}
	}
	return out
}

// All returns a snapshot of every node in the ring regardless of status.
func (r *NodeRing) All() []NodeInfo {
	r.mu.RLock()
	defer r.mu.RUnlock()
	result := make([]NodeInfo, len(r.nodes))
	copy(result, r.nodes)
	return result
}

// Size returns the total number of nodes currently in the ring.
func (r *NodeRing) Size() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.nodes)
}

// UpdateStatus sets the Status and LastSeen fields for the node with nodeID.
// A no-op if no node with that ID exists.
func (r *NodeRing) UpdateStatus(nodeID string, status NodeStatus, lastSeen time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for i := range r.nodes {
		if r.nodes[i].ID == nodeID {
			r.nodes[i].Status = status
			r.nodes[i].LastSeen = lastSeen
			return
		}
	}
}

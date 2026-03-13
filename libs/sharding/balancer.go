package sharding

import (
	"context"
	"fmt"
	"sync"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/connectivity"
	"google.golang.org/grpc/credentials/insecure"
)

// ShardAwareLoadBalancer routes gRPC connections to the correct shard node
// based on account IDs. Connections are created lazily on first use and cached
// by node address. Safe for concurrent use.
type ShardAwareLoadBalancer struct {
	router   ShardRouter
	ring     *NodeRing
	connPool map[string]*grpc.ClientConn
	mu       sync.RWMutex
}

// NewShardAwareLoadBalancer constructs a ShardAwareLoadBalancer using the
// provided router for shard lookup and ring for healthy-node fallback.
func NewShardAwareLoadBalancer(router ShardRouter, ring *NodeRing) *ShardAwareLoadBalancer {
	return &ShardAwareLoadBalancer{
		router:   router,
		ring:     ring,
		connPool: make(map[string]*grpc.ClientConn),
	}
}

// GetConnection returns the gRPC connection to the node responsible for
// accountID. If the primary shard node is down, the balancer falls back to
// any other healthy node in the ring. The connection is created lazily.
func (b *ShardAwareLoadBalancer) GetConnection(accountID uint64) (*grpc.ClientConn, error) {
	node, err := b.router.RouteByAccount(accountID)
	if err != nil {
		// Primary node for this shard is down — fall back to any healthy node.
		healthy := b.ring.Healthy()
		if len(healthy) == 0 {
			return nil, fmt.Errorf("sharding: no healthy nodes available for account %d", accountID)
		}
		node = healthy[0]
	}
	return b.getOrDialConn(node.Address)
}

// GetConnectionForShard returns the gRPC connection to the node owning
// shardID. Returns an error if the shard has no healthy node.
func (b *ShardAwareLoadBalancer) GetConnectionForShard(shardID int) (*grpc.ClientConn, error) {
	node, err := b.router.RouteByShardID(shardID)
	if err != nil {
		return nil, err
	}
	return b.getOrDialConn(node.Address)
}

// HealthCheck iterates all nodes in the ring and marks any whose cached gRPC
// connection is in TransientFailure or Shutdown state as NodeStatusDegraded.
func (b *ShardAwareLoadBalancer) HealthCheck(_ context.Context) error {
	nodes := b.ring.All()
	now := time.Now().UTC()
	for _, node := range nodes {
		b.mu.RLock()
		conn, ok := b.connPool[node.Address]
		b.mu.RUnlock()
		if !ok {
			continue
		}
		state := conn.GetState()
		if state == connectivity.TransientFailure || state == connectivity.Shutdown {
			b.ring.UpdateStatus(node.ID, NodeStatusDegraded, now)
		}
	}
	return nil
}

// Close closes all cached gRPC connections and empties the connection pool.
func (b *ShardAwareLoadBalancer) Close() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	var firstErr error
	for addr, conn := range b.connPool {
		if err := conn.Close(); err != nil && firstErr == nil {
			firstErr = fmt.Errorf("sharding: close %s: %w", addr, err)
		}
	}
	b.connPool = make(map[string]*grpc.ClientConn)
	return firstErr
}

// getOrDialConn returns a cached connection for addr, or dials a new lazy
// (non-blocking) gRPC connection and caches it.
func (b *ShardAwareLoadBalancer) getOrDialConn(addr string) (*grpc.ClientConn, error) {
	// Fast path: read lock.
	b.mu.RLock()
	if conn, ok := b.connPool[addr]; ok {
		b.mu.RUnlock()
		return conn, nil
	}
	b.mu.RUnlock()

	// Slow path: dial and cache under write lock.
	b.mu.Lock()
	defer b.mu.Unlock()
	// Double-check after acquiring write lock.
	if conn, ok := b.connPool[addr]; ok {
		return conn, nil
	}
	//nolint:staticcheck // grpc.Dial is the stable API in grpc v1.64.0
	conn, err := grpc.Dial(addr,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		return nil, fmt.Errorf("sharding: dial %s: %w", addr, err)
	}
	b.connPool[addr] = conn
	return conn, nil
}

package discovery

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
)

// StaticRegistry is a ServiceRegistry backed by the BLAZIL_NODES environment
// variable. It is read once at construction time and never updated, making it
// suitable for Docker Compose and fixed-topology DigitalOcean deployments where
// the cluster membership is known at startup.
//
// BLAZIL_NODES format:
//
//	<nodeID>:<host>:<port>[,<nodeID>:<host>:<port>...]
//
// Example:
//
//	BLAZIL_NODES=node-1:10.0.0.1:7878,node-2:10.0.0.2:7878,node-3:10.0.0.3:7878
//
// All services on each node are reachable at the same host with service-specific
// ports configured separately. The engine address from BLAZIL_NODES is used for
// TCP health checking; individual service addresses use different ports.
type StaticRegistry struct {
	mu        sync.RWMutex
	// services maps service name → registered endpoints.
	services  map[string][]NodeEndpoint
	// nodes holds the raw node list parsed from BLAZIL_NODES.
	nodes     []nodeEntry
}

type nodeEntry struct {
	nodeID  string
	address string
	shardID int
}

// NewStaticRegistry constructs a StaticRegistry by reading BLAZIL_NODES from
// the environment. Returns an error if the variable is empty or malformed.
func NewStaticRegistry() (*StaticRegistry, error) {
	raw := os.Getenv("BLAZIL_NODES")
	if raw == "" {
		return nil, fmt.Errorf("discovery: BLAZIL_NODES is not set")
	}
	entries, err := parseNodes(raw)
	if err != nil {
		return nil, fmt.Errorf("discovery: parse BLAZIL_NODES: %w", err)
	}
	return &StaticRegistry{
		services: make(map[string][]NodeEndpoint),
		nodes:    entries,
	}, nil
}

// parseNodes parses the BLAZIL_NODES value into a slice of nodeEntry.
// Format: node-1:host1:7878,node-2:host2:7879
func parseNodes(raw string) ([]nodeEntry, error) {
	parts := strings.Split(raw, ",")
	entries := make([]nodeEntry, 0, len(parts))
	for i, p := range parts {
		p = strings.TrimSpace(p)
		if p == "" {
			continue
		}
		fields := strings.SplitN(p, ":", 3)
		if len(fields) != 3 {
			return nil, fmt.Errorf("entry %d %q: expected nodeID:host:port", i, p)
		}
		nodeID, host, port := fields[0], fields[1], fields[2]
		if nodeID == "" || host == "" || port == "" {
			return nil, fmt.Errorf("entry %d %q: nodeID, host, and port must not be empty", i, p)
		}
		// ShardID is derived from position (0-indexed) unless a shard tag is
		// embedded in the node ID (e.g. "node-1" → shardID 0).
		// For simplicity we use the position index as the shard ID; callers
		// that need explicit sharding override via Register.
		entries = append(entries, nodeEntry{
			nodeID:  nodeID,
			address: host + ":" + port,
			shardID: i,
		})
	}
	if len(entries) == 0 {
		return nil, fmt.Errorf("no valid entries found")
	}
	return entries, nil
}

// Register adds an endpoint for the given service. Static registries accept
// explicit registrations in addition to the entries parsed from BLAZIL_NODES,
// which allows callers to register service-specific addresses (e.g. payments
// on port 50051 rather than the engine on port 7878).
func (r *StaticRegistry) Register(_ context.Context, node NodeRegistration) error {
	ep := NodeEndpoint{
		NodeID:  node.NodeID,
		Address: node.Address,
		ShardID: node.ShardID,
		Healthy: true,
	}
	r.mu.Lock()
	r.services[node.Service] = append(r.services[node.Service], ep)
	r.mu.Unlock()
	return nil
}

// Deregister removes all endpoints for nodeID across all services.
func (r *StaticRegistry) Deregister(_ context.Context, nodeID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	for service, eps := range r.services {
		var kept []NodeEndpoint
		for _, ep := range eps {
			if ep.NodeID != nodeID {
				kept = append(kept, ep)
			}
		}
		r.services[service] = kept
	}
	return nil
}

// Discover returns all registered endpoints for the given service. If no
// endpoints are registered under that service name, it falls back to the
// node list parsed from BLAZIL_NODES with a synthesised address for the
// service (host:defaultPort). Returns an error if no nodes are known at all.
func (r *StaticRegistry) Discover(_ context.Context, service string) ([]NodeEndpoint, error) {
	r.mu.RLock()
	eps, ok := r.services[service]
	r.mu.RUnlock()
	if ok && len(eps) > 0 {
		// Return a copy to prevent callers from mutating the slice.
		out := make([]NodeEndpoint, len(eps))
		copy(out, eps)
		return out, nil
	}
	// Fall back to the raw node list (engine addresses).
	if len(r.nodes) == 0 {
		return nil, fmt.Errorf("discovery: no nodes registered for service %q", service)
	}
	out := make([]NodeEndpoint, len(r.nodes))
	for i, n := range r.nodes {
		out[i] = NodeEndpoint{
			NodeID:  n.nodeID,
			Address: n.address,
			ShardID: n.shardID,
			Healthy: true,
		}
	}
	return out, nil
}

// Watch sends the current endpoint list to ch once, then blocks until the
// context is cancelled. The StaticRegistry never emits further updates because
// the topology is fixed.
func (r *StaticRegistry) Watch(ctx context.Context, service string, ch chan<- []NodeEndpoint) error {
	eps, err := r.Discover(ctx, service)
	if err != nil {
		return err
	}
	ch <- eps
	<-ctx.Done()
	return nil
}

// defaultPort returns a default service port for well-known service names.
// Used only in the BLAZIL_NODES fallback path.
func defaultPort(service string) string {
	defaults := map[string]string{
		"engine":   "7878",
		"payments": "50051",
		"banking":  "50052",
		"trading":  "50053",
		"crypto":   "50054",
	}
	if p, ok := defaults[service]; ok {
		return p
	}
	return "7878"
}

// shardFromNodeID extracts a numeric shard from a node ID like "node-1".
// Falls back to 0 on parse failure.
func shardFromNodeID(nodeID string) int {
	parts := strings.Split(nodeID, "-")
	if len(parts) < 2 {
		return 0
	}
	n, err := strconv.Atoi(parts[len(parts)-1])
	if err != nil {
		return 0
	}
	return n - 1 // "node-1" → shard 0
}

// Package discovery provides service registration and discovery for the Blazil
// multi-node cluster. It is intentionally minimal: no Consul, ZooKeeper, or
// etcd. A StaticRegistry backed by environment variables is sufficient for the
// fixed-topology deployments targeted by Blazil (Docker Compose, DigitalOcean).
package discovery

import "context"

// NodeRegistration describes a service instance being registered with the
// registry.
type NodeRegistration struct {
	// NodeID is the unique human-readable node identifier (e.g. "node-1").
	NodeID string
	// Service is the logical service name (e.g. "engine", "payments").
	Service string
	// Address is the reachable network address (e.g. "10.0.0.1:7878").
	Address string
	// ShardID is the shard this node owns.
	ShardID int
	// Tags holds arbitrary key-value metadata.
	Tags map[string]string
}

// NodeEndpoint is a discovered service instance returned by Discover or Watch.
type NodeEndpoint struct {
	// NodeID is the unique human-readable node identifier.
	NodeID string
	// Address is the reachable network address.
	Address string
	// ShardID is the shard this node owns.
	ShardID int
	// Healthy indicates whether the most recent health check passed.
	Healthy bool
}

// ServiceRegistry handles registration, deregistration and discovery of service
// endpoints. Implementations must be safe for concurrent use.
type ServiceRegistry interface {
	// Register announces a service instance to the registry.
	Register(ctx context.Context, node NodeRegistration) error

	// Deregister removes the service instance identified by nodeID.
	Deregister(ctx context.Context, nodeID string) error

	// Discover returns all currently known endpoints for the given service.
	// Returns an error if the registry is unavailable or the service is
	// unknown.
	Discover(ctx context.Context, service string) ([]NodeEndpoint, error)

	// Watch sends the current endpoint list to ch, then blocks until the
	// context is cancelled. Static implementations send the initial list once
	// and then block (no further updates). Dynamic implementations re-send
	// on topology changes.
	Watch(ctx context.Context, service string, ch chan<- []NodeEndpoint) error
}

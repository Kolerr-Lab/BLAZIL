package sharding

import (
	"context"
	"fmt"
	"sync"
)

// CrossShardRequest describes an atomic transfer between accounts that reside
// on different shard nodes.
type CrossShardRequest struct {
	// FromAccountID is the uint64-hashed debit account ID.
	FromAccountID uint64
	// ToAccountID is the uint64-hashed credit account ID.
	ToAccountID uint64
	// Amount is the transfer amount in the currency's minor units.
	Amount int64
	// Currency is the ISO 4217 currency code (e.g. "USD").
	Currency string
	// IdempotencyKey is a client-assigned unique key for deduplication.
	IdempotencyKey string
}

// NodeTransferClient abstracts the engine-level transfer submission API for a
// single node. In production this wraps the node engine's gRPC stub; in tests
// it is replaced by a mock.
type NodeTransferClient interface {
	// SubmitPending reserves funds on the source account via a TigerBeetle
	// linked pending transfer. Returns a transferID for Commit or Void.
	SubmitPending(ctx context.Context, req CrossShardRequest) (transferID string, err error)
	// Commit releases funds on the destination account by posting the
	// pending transfer.
	Commit(ctx context.Context, transferID string) error
	// Void cancels a pending transfer, releasing the reserved funds back to
	// the source account.
	Void(ctx context.Context, transferID string) error
}

// NodeTransferClientFactory creates a NodeTransferClient for the given node
// address. In production this wraps the engine gRPC stub for that address. In
// tests it returns a mock keyed by address.
type NodeTransferClientFactory func(nodeAddress string) NodeTransferClient

// CrossShardCoordinator executes atomic transfers that span two shard nodes.
// It uses TigerBeetle linked transfers (pending → commit / void) to achieve
// atomicity at the storage layer without a custom 2PC protocol.
//
// Both node engines connect to the same TigerBeetle cluster, so a pending
// transfer submitted via the source engine and committed via the destination
// engine is safe — TigerBeetle guarantees atomicity at the ledger level.
type CrossShardCoordinator interface {
	Execute(ctx context.Context, req CrossShardRequest) error
}

// CrossShardCoordinatorImpl is the production implementation of
// CrossShardCoordinator. It is safe for concurrent use.
type CrossShardCoordinatorImpl struct {
	router  ShardRouter
	balancer *ShardAwareLoadBalancer
	factory NodeTransferClientFactory
	// cache stores execution outcomes keyed by IdempotencyKey.
	// Values are errors (or nil for success).
	cache sync.Map
}

// NewCrossShardCoordinator constructs a CrossShardCoordinatorImpl.
// factory is used to obtain a NodeTransferClient for each node address;
// it must not be nil.
func NewCrossShardCoordinator(
	router ShardRouter,
	balancer *ShardAwareLoadBalancer,
	factory NodeTransferClientFactory,
) *CrossShardCoordinatorImpl {
	return &CrossShardCoordinatorImpl{
		router:  router,
		balancer: balancer,
		factory: factory,
	}
}

// Execute implements CrossShardCoordinator.
//
// Flow:
//  1. Validate that the two accounts reside on different shards.
//  2. Check the idempotency cache; return the cached outcome on a hit.
//  3. Route each account to its owning node.
//  4. Submit a PENDING transfer on the source node engine.
//  5. Submit a COMMIT transfer on the destination node engine.
//  6. If COMMIT fails, submit VOID on the source node (best-effort).
//  7. Cache the outcome and return.
func (c *CrossShardCoordinatorImpl) Execute(ctx context.Context, req CrossShardRequest) error {
	// Step 1 — validate: must be a genuine cross-shard transfer.
	if !c.router.IsCrossShard(req.FromAccountID, req.ToAccountID) {
		return fmt.Errorf(
			"crossshard: accounts %d and %d are on the same shard; use the normal transfer path",
			req.FromAccountID, req.ToAccountID,
		)
	}

	// Step 2 — idempotency cache lookup.
	if cached, ok := c.cache.Load(req.IdempotencyKey); ok {
		if err, _ := cached.(error); err != nil {
			return err
		}
		return nil
	}

	// Step 3 — route source and destination accounts to their nodes.
	sourceNode, err := c.router.RouteByAccount(req.FromAccountID)
	if err != nil {
		return fmt.Errorf("crossshard: route source account %d: %w", req.FromAccountID, err)
	}
	destNode, err := c.router.RouteByAccount(req.ToAccountID)
	if err != nil {
		return fmt.Errorf("crossshard: route dest account %d: %w", req.ToAccountID, err)
	}

	sourceClient := c.factory(sourceNode.Address)
	destClient := c.factory(destNode.Address)

	// Step 4 — submit PENDING on source node.
	transferID, err := sourceClient.SubmitPending(ctx, req)
	if err != nil {
		finalErr := fmt.Errorf("crossshard: submit pending on %s: %w", sourceNode.Address, err)
		c.cache.Store(req.IdempotencyKey, finalErr)
		return finalErr
	}

	// Step 5 — submit COMMIT on dest node.
	if err := destClient.Commit(ctx, transferID); err != nil {
		// Step 6 — commit failed: void the pending transfer on the source (best-effort).
		_ = sourceClient.Void(ctx, transferID)
		finalErr := fmt.Errorf("crossshard: commit failed on %s (void attempted): %w", destNode.Address, err)
		c.cache.Store(req.IdempotencyKey, finalErr)
		return finalErr
	}

	// Step 7 — success.
	c.cache.Store(req.IdempotencyKey, (error)(nil))
	return nil
}

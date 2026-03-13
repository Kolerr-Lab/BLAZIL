// Package engine provides the Blazil engine client implementations.
package engine

import (
	"context"
	"fmt"
	"time"

	"github.com/blazil/sharding"
	"google.golang.org/grpc"
)

// tcpTransferClient implements sharding.NodeTransferClient over the existing
// TCP connection established by the engine.  The full TigerBeetle linked-
// transfer flow (pending → commit / void) is wired in Prompt 18; this stub
// preserves the correct interface so the cross-shard coordinator can be wired
// today without blocking on the full implementation.
type tcpTransferClient struct {
	addr    string
	timeout time.Duration
}

// NewTcpTransferClient returns a sharding.NodeTransferClient that targets the
// Blazil engine at conn.Target().  The caller retains ownership of conn; the
// returned client does not close it.
//
// NOTE (Prompt 18): Implement SubmitPending / Commit / Void via TigerBeetle
// linked transfers once the engine TCP protocol exposes those operations.
func NewTcpTransferClient(conn *grpc.ClientConn) sharding.NodeTransferClient {
	return &tcpTransferClient{
		addr:    conn.Target(),
		timeout: 5 * time.Second,
	}
}

// SubmitPending satisfies sharding.NodeTransferClient.  Returns a synthetic
// transferID built from the idempotency key so the coordinator can track the
// operation.  Full TigerBeetle pending-transfer support is deferred to
// Prompt 18.
func (c *tcpTransferClient) SubmitPending(_ context.Context, req sharding.CrossShardRequest) (string, error) {
	return fmt.Sprintf("xshard-%s", req.IdempotencyKey), nil
}

// Commit satisfies sharding.NodeTransferClient.  No-op until Prompt 18.
func (c *tcpTransferClient) Commit(_ context.Context, _ string) error { return nil }

// Void satisfies sharding.NodeTransferClient.  No-op until Prompt 18.
func (c *tcpTransferClient) Void(_ context.Context, _ string) error { return nil }

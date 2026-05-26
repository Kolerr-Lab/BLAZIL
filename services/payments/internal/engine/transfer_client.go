// Package engine provides the Blazil engine client implementations.
package engine

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/vmihailenco/msgpack/v5"

	"github.com/blazil/sharding"
	"google.golang.org/grpc"
)

// Flag constants that mirror EventFlags in the Rust engine.
const (
	flagPending = uint8(0x02) // reserve funds via pending transfer
	flagPost    = uint8(0x08) // commit: post the pending transfer
	flagVoid    = uint8(0x10) // abort: void the pending transfer
)

// pendingInfo holds the fields needed to post or void a pending transfer.
type pendingInfo struct {
	debitAccountID  string
	creditAccountID string
	amount          string // decimal-formatted minor units (e.g. "10000")
	currency        string
	ledgerID        uint32
}

// tcpTransferClient implements sharding.NodeTransferClient over the existing
// TCP engine connection using the length-prefixed MessagePack protocol.
//
// Phase 1 (SubmitPending) submits a TigerBeetle pending transfer and stores
// the account/amount metadata so Phase 2 (Commit or Void) can reconstruct the
// full request without re-querying the coordinator.
type tcpTransferClient struct {
	addr           string
	timeout        time.Duration
	poolOnce       sync.Once
	pool           *ConnectionPool
	pendingAmounts sync.Map // key: transferID string → value: pendingInfo
}

// NewTcpTransferClient returns a sharding.NodeTransferClient that targets the
// Blazil engine at conn.Target().  The caller retains ownership of conn; the
// returned client does not close it.
func NewTcpTransferClient(conn *grpc.ClientConn) sharding.NodeTransferClient {
	return &tcpTransferClient{
		addr:    conn.Target(),
		timeout: 5 * time.Second,
	}
}

// getPool returns the lazily-initialised connection pool.
// A single connection is enough for 2PC operations (low frequency).
func (c *tcpTransferClient) getPool(ctx context.Context) (*ConnectionPool, error) {
	var initErr error
	c.poolOnce.Do(func() {
		p, err := NewConnectionPool(c.addr, 1, c.timeout)
		if err != nil {
			initErr = err
			return
		}
		c.pool = p
	})
	if initErr != nil {
		return nil, fmt.Errorf("transfer client: pool init failed: %w", initErr)
	}
	if c.pool == nil {
		return nil, fmt.Errorf("transfer client: connection pool not initialised")
	}
	return c.pool, nil
}

// currencyToLedgerID maps an ISO 4217 currency code to its Blazil ledger ID.
// Values must match the LedgerId constants in blazil_common::ids.
func currencyToLedgerID(currency string) uint32 {
	switch currency {
	case "USD":
		return 1
	case "EUR":
		return 2
	case "GBP":
		return 3
	case "JPY":
		return 4
	case "VND":
		return 5
	case "BTC":
		return 6
	case "ETH":
		return 7
	default:
		return 1 // default to USD ledger
	}
}

// marshalTransactionRequest serialises a transactionRequest directly to
// MessagePack bytes, used by the 2PC path where no domain.Payment is involved.
func marshalTransactionRequest(req transactionRequest) ([]byte, error) {
	b, err := msgpack.Marshal(&req)
	if err != nil {
		return nil, fmt.Errorf("transfer client: failed to marshal request: %w", err)
	}
	return b, nil
}

// sendRequest serialises req as a length-prefixed MessagePack frame and sends
// it to the engine, returning (committed, transferID, error).
func (c *tcpTransferClient) sendRequest(ctx context.Context, req transactionRequest) (string, error) {
	b, err := marshalTransactionRequest(req)
	if err != nil {
		return "", err
	}

	pool, err := c.getPool(ctx)
	if err != nil {
		return "", err
	}

	conn, err := pool.Acquire(ctx)
	if err != nil {
		return "", fmt.Errorf("transfer client: %w", err)
	}

	if deadline, ok := ctx.Deadline(); ok {
		conn.SetDeadline(deadline) //nolint:errcheck
	}

	committed, transferID, connErr := sendReceive(conn, b)
	pool.Release(conn, connErr)
	if connErr != nil {
		return "", fmt.Errorf("transfer client: %w", connErr)
	}
	if !committed {
		return "", fmt.Errorf("transfer client: engine rejected 2PC request")
	}
	return transferID, nil
}

// SubmitPending submits a TigerBeetle pending transfer to the engine.
// The transfer reserves funds on the source account without moving them.
// Returns the transfer ID assigned by TigerBeetle to use in Commit or Void.
func (c *tcpTransferClient) SubmitPending(ctx context.Context, req sharding.CrossShardRequest) (string, error) {
	ledgerID := currencyToLedgerID(req.Currency)
	amount := fmt.Sprintf("%d", req.Amount)
	debitID := fmt.Sprintf("%d", req.FromAccountID)
	creditID := fmt.Sprintf("%d", req.ToAccountID)

	engineReq := transactionRequest{
		RequestID:         uuid.New().String(),
		DebitAccountID:    debitID,
		CreditAccountID:   creditID,
		Amount:            amount,
		Currency:          req.Currency,
		LedgerID:          ledgerID,
		Code:              1,
		Flags:             flagPending,
		PendingTransferID: "",
	}

	transferID, err := c.sendRequest(ctx, engineReq)
	if err != nil {
		return "", fmt.Errorf("SubmitPending: %w", err)
	}

	// Store the metadata so Commit/Void can reconstruct the request.
	c.pendingAmounts.Store(transferID, pendingInfo{
		debitAccountID:  debitID,
		creditAccountID: creditID,
		amount:          amount,
		currency:        req.Currency,
		ledgerID:        ledgerID,
	})

	return transferID, nil
}

// Commit posts a previously submitted pending transfer, finalising the funds
// movement from the source to the destination account.
func (c *tcpTransferClient) Commit(ctx context.Context, transferID string) error {
	info, ok := c.pendingAmounts.LoadAndDelete(transferID)
	if !ok {
		return fmt.Errorf("Commit: no pending transfer found for ID %q", transferID)
	}
	p := info.(pendingInfo)

	engineReq := transactionRequest{
		RequestID:         uuid.New().String(),
		DebitAccountID:    p.debitAccountID,
		CreditAccountID:   p.creditAccountID,
		Amount:            p.amount,
		Currency:          p.currency,
		LedgerID:          p.ledgerID,
		Code:              1,
		Flags:             flagPost,
		PendingTransferID: transferID,
	}

	if _, err := c.sendRequest(ctx, engineReq); err != nil {
		return fmt.Errorf("Commit: %w", err)
	}
	return nil
}

// Void cancels a pending transfer, releasing the reserved funds back to the
// source account.
func (c *tcpTransferClient) Void(ctx context.Context, transferID string) error {
	info, ok := c.pendingAmounts.LoadAndDelete(transferID)
	if !ok {
		return fmt.Errorf("Void: no pending transfer found for ID %q", transferID)
	}
	p := info.(pendingInfo)

	engineReq := transactionRequest{
		RequestID:         uuid.New().String(),
		DebitAccountID:    p.debitAccountID,
		CreditAccountID:   p.creditAccountID,
		Amount:            p.amount,
		Currency:          p.currency,
		LedgerID:          p.ledgerID,
		Code:              1,
		Flags:             flagVoid,
		PendingTransferID: transferID,
	}

	if _, err := c.sendRequest(ctx, engineReq); err != nil {
		return fmt.Errorf("Void: %w", err)
	}
	return nil
}

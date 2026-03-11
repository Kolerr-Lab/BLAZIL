// Package engine provides the client interface for the Blazil Rust engine.
package engine

import (
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"time"

	"github.com/vmihailenco/msgpack/v5"

	"github.com/blazil/services/payments/internal/domain"
)

// BlazerEngineClient submits a payment to the Blazil Rust transport layer.
// Implementations must be safe for concurrent use.
type BlazerEngineClient interface {
	// Submit sends the payment to the Rust engine and returns whether it was
	// committed, the assigned transfer ID, and any infrastructure error.
	Submit(ctx context.Context, p *domain.Payment) (committed bool, transferID string, err error)
}

// ── Wire protocol types ───────────────────────────────────────────────────────
// These mirror the Rust TransactionRequest / TransactionResponse structs.
// Encoded as MessagePack arrays (compact format, matching rmp_serde::to_vec).

// transactionRequest is the Go representation of the Rust TransactionRequest.
// Field order must exactly match the Rust struct definition to ensure correct
// MessagePack array encoding/decoding.
type transactionRequest struct {
	RequestID       string `msgpack:"request_id"`
	DebitAccountID  string `msgpack:"debit_account_id"`
	CreditAccountID string `msgpack:"credit_account_id"`
	Amount          string `msgpack:"amount"`
	Currency        string `msgpack:"currency"`
	LedgerID        uint32 `msgpack:"ledger_id"`
	Code            uint16 `msgpack:"code"`
}

// transactionResponse is the Go representation of the Rust TransactionResponse.
type transactionResponse struct {
	RequestID   string  `msgpack:"request_id"`
	Committed   bool    `msgpack:"committed"`
	TransferID  *string `msgpack:"transfer_id"`
	Error       *string `msgpack:"error"`
	TimestampNs uint64  `msgpack:"timestamp_ns"`
}

// ── ConnectionPool ────────────────────────────────────────────────────────────

// ConnectionPool maintains a bounded set of persistent TCP connections to the
// Blazil Rust engine. Connections are pre-dialed on construction and returned
// to the pool after each use.
//
// Safe for concurrent use by multiple goroutines.
type ConnectionPool struct {
	addr        string
	conns       chan net.Conn
	maxSize     int
	dialTimeout time.Duration
}

// NewConnectionPool dials maxSize TCP connections to addr and returns the pool.
// Returns an error if not a single connection can be established (the engine
// is unreachable). Partial success is accepted: if at least one connection
// succeeds the pool is usable.
func NewConnectionPool(addr string, maxSize int, dialTimeout time.Duration) (*ConnectionPool, error) {
	if maxSize <= 0 {
		maxSize = 1
	}
	pool := &ConnectionPool{
		addr:        addr,
		conns:       make(chan net.Conn, maxSize),
		maxSize:     maxSize,
		dialTimeout: dialTimeout,
	}

	established := 0
	for i := 0; i < maxSize; i++ {
		conn, err := net.DialTimeout("tcp", addr, dialTimeout)
		if err != nil {
			continue // best-effort pre-dial
		}
		pool.conns <- conn
		established++
	}

	if established == 0 {
		return nil, fmt.Errorf("connection pool: could not establish any connection to %s", addr)
	}
	return pool, nil
}

// Acquire obtains a connection from the pool.
// Blocks until one is available or ctx is cancelled.
func (p *ConnectionPool) Acquire(ctx context.Context) (net.Conn, error) {
	select {
	case conn := <-p.conns:
		return conn, nil
	case <-ctx.Done():
		return nil, fmt.Errorf("connection pool: acquire cancelled: %w", ctx.Err())
	}
}

// Release returns a connection to the pool.
// If the connection is broken (passed err is non-nil), it is closed and a
// replacement is dialled best-effort. The replacement (or nothing, if the
// dial fails) is placed back in the pool without blocking.
func (p *ConnectionPool) Release(conn net.Conn, connErr error) {
	if connErr != nil {
		conn.Close() //nolint:errcheck
		// Best-effort replacement dial — do not block the caller.
		go func() {
			replacement, err := net.DialTimeout("tcp", p.addr, p.dialTimeout)
			if err != nil {
				return
			}
			// Non-blocking send: if the pool is already full (shouldn't happen
			// in normal operation), discard the replacement rather than blocking.
			select {
			case p.conns <- replacement:
			default:
				replacement.Close() //nolint:errcheck
			}
		}()
		return
	}
	select {
	case p.conns <- conn:
	default:
		// Pool is somehow full — close the excess connection.
		conn.Close() //nolint:errcheck
	}
}

// Close drains the pool and closes all idle connections.
func (p *ConnectionPool) Close() {
	for {
		select {
		case conn := <-p.conns:
			conn.Close() //nolint:errcheck
		default:
			return
		}
	}
}

// NewEmptyPool returns a ConnectionPool with no pre-dialled connections.
// Useful in tests for exercising context-cancellation paths without requiring
// a live server.
func NewEmptyPool(addr string, dialTimeout time.Duration) *ConnectionPool {
	return &ConnectionPool{
		addr:        addr,
		conns:       make(chan net.Conn, 10),
		maxSize:     10,
		dialTimeout: dialTimeout,
	}
}

// ── TcpEngineClient ───────────────────────────────────────────────────────────

// TcpEngineClient submits payments to the Blazil Rust engine over TCP using the
// 4-byte big-endian length-prefixed MessagePack protocol.
//
// Connections are managed by a ConnectionPool to avoid per-call dial overhead
// and file-descriptor exhaustion under load.
type TcpEngineClient struct {
	pool    *ConnectionPool
	timeout time.Duration
}

// NewTcpEngineClient constructs a TcpEngineClient with a pre-dialled connection
// pool of size poolSize targeting addr (e.g. "127.0.0.1:7878").
func NewTcpEngineClient(addr string, timeout time.Duration) *TcpEngineClient {
	// Use a shared pool of 10 connections. Errors here are non-fatal at
	// construction time; Submit will fail when the engine is unreachable.
	pool, _ := NewConnectionPool(addr, 10, timeout)
	if pool == nil {
		// Engine unreachable at startup — create a zero-connection sentinel pool
		// so the struct is valid; all Submit calls will fail gracefully.
		pool = &ConnectionPool{
			addr:        addr,
			conns:       make(chan net.Conn, 0),
			maxSize:     10,
			dialTimeout: timeout,
		}
	}
	return &TcpEngineClient{pool: pool, timeout: timeout}
}

// Submit implements BlazerEngineClient.
func (c *TcpEngineClient) Submit(ctx context.Context, p *domain.Payment) (bool, string, error) {
	payload, err := marshalRequest(p)
	if err != nil {
		return false, "", err
	}

	conn, err := c.pool.Acquire(ctx)
	if err != nil {
		return false, "", fmt.Errorf("engine client: %w", err)
	}

	if deadline, ok := ctx.Deadline(); ok {
		conn.SetDeadline(deadline) //nolint:errcheck
	}

	committed, transferID, connErr := sendReceive(conn, payload)
	c.pool.Release(conn, connErr)
	if connErr != nil {
		return false, "", fmt.Errorf("engine client: %w", connErr)
	}
	return committed, transferID, nil
}

// marshalRequest serialises a Payment into a MessagePack wire payload.
func marshalRequest(p *domain.Payment) ([]byte, error) {
	req := transactionRequest{
		RequestID:       string(p.ID),
		DebitAccountID:  string(p.DebitAccountID),
		CreditAccountID: string(p.CreditAccountID),
		Amount:          fmt.Sprintf("%d", p.Amount.MinorUnits),
		Currency:        p.Amount.Currency.Code,
		LedgerID:        uint32(p.LedgerID),
		Code:            1,
	}
	b, err := msgpack.Marshal(&req)
	if err != nil {
		return nil, fmt.Errorf("engine client: failed to marshal request: %w", err)
	}
	return b, nil
}

// sendReceive writes a length-prefixed payload to conn and reads back a response.
// Returns the committed flag, transfer ID, and any I/O error.
// The returned error signals whether the connection should be retired.
func sendReceive(conn net.Conn, payload []byte) (bool, string, error) {
	header := make([]byte, 4)
	binary.BigEndian.PutUint32(header, uint32(len(payload)))
	if _, err := conn.Write(append(header, payload...)); err != nil {
		return false, "", fmt.Errorf("failed to write frame: %w", err)
	}

	if _, err := io.ReadFull(conn, header); err != nil {
		return false, "", fmt.Errorf("failed to read response header: %w", err)
	}
	respLen := binary.BigEndian.Uint32(header)
	if respLen > 1_048_576 {
		return false, "", fmt.Errorf("response frame too large: %d bytes", respLen)
	}

	respPayload := make([]byte, respLen)
	if _, err := io.ReadFull(conn, respPayload); err != nil {
		return false, "", fmt.Errorf("failed to read response payload: %w", err)
	}

	var resp transactionResponse
	if err := msgpack.Unmarshal(respPayload, &resp); err != nil {
		return false, "", fmt.Errorf("failed to unmarshal response: %w", err)
	}
	if resp.Error != nil {
		return false, "", fmt.Errorf("server error: %s", *resp.Error)
	}

	transferID := ""
	if resp.TransferID != nil {
		transferID = *resp.TransferID
	}
	return resp.Committed, transferID, nil
}

// ── MockEngineClient ──────────────────────────────────────────────────────────

// MockEngineClient is a test double for BlazerEngineClient.
// By default it returns committed=true with a deterministic transfer ID.
type MockEngineClient struct {
	// ReturnError, if non-nil, is returned from every Submit call.
	ReturnError error

	// ReturnCommitted controls whether Submit reports the transaction as committed.
	// Defaults to true.
	ReturnCommitted bool

	// Calls records the payments submitted so tests can inspect them.
	Calls []*domain.Payment
}

// NewMockEngineClient returns a MockEngineClient configured to approve everything.
func NewMockEngineClient() *MockEngineClient {
	return &MockEngineClient{ReturnCommitted: true}
}

// Submit implements BlazerEngineClient.
func (m *MockEngineClient) Submit(_ context.Context, p *domain.Payment) (bool, string, error) {
	m.Calls = append(m.Calls, p)
	if m.ReturnError != nil {
		return false, "", m.ReturnError
	}
	transferID := "mock-transfer-" + string(p.ID)
	return m.ReturnCommitted, transferID, nil
}

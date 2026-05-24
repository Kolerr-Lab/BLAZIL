package engine

import (
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"time"

	"github.com/vmihailenco/msgpack/v5"
)

// engineRequest is the wire format sent to the Rust engine.
type engineRequest struct {
	RequestID string `msgpack:"request_id"`
	AccountID string `msgpack:"account_id"`
	Amount    int64  `msgpack:"amount"`
	Op        string `msgpack:"op"` // "debit" or "credit"
}

// engineResponse is the wire format received from the Rust engine.
type engineResponse struct {
	RequestID string  `msgpack:"request_id"`
	OK        bool    `msgpack:"ok"`
	Error     *string `msgpack:"error"`
}

// TcpEngineClient sends debit/credit operations to the Blazil Rust engine
// over a pool of persistent TCP connections using MessagePack framing.
//
// Safe for concurrent use.
type TcpEngineClient struct {
	addr        string
	conns       chan net.Conn
	dialTimeout time.Duration
	reqTimeout  time.Duration
}

// NewTcpEngineClient dials maxSize TCP connections to addr and returns the client.
func NewTcpEngineClient(addr string, maxSize int, dialTimeout time.Duration) (*TcpEngineClient, error) {
	if maxSize <= 0 {
		maxSize = 4
	}
	c := &TcpEngineClient{
		addr:        addr,
		conns:       make(chan net.Conn, maxSize),
		dialTimeout: dialTimeout,
		reqTimeout:  5 * time.Second,
	}
	established := 0
	for i := 0; i < maxSize; i++ {
		conn, err := net.DialTimeout("tcp", addr, dialTimeout)
		if err != nil {
			continue
		}
		c.conns <- conn
		established++
	}
	if established == 0 {
		return nil, fmt.Errorf("TcpEngineClient: could not connect to engine at %s", addr)
	}
	return c, nil
}

func (c *TcpEngineClient) acquire(ctx context.Context) (net.Conn, error) {
	select {
	case conn := <-c.conns:
		return conn, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

func (c *TcpEngineClient) release(conn net.Conn, healthy bool) {
	if !healthy {
		conn.Close()
		// Re-dial in background.
		go func() {
			newConn, err := net.DialTimeout("tcp", c.addr, c.dialTimeout)
			if err == nil {
				c.conns <- newConn
			}
		}()
		return
	}
	c.conns <- conn
}

func (c *TcpEngineClient) call(ctx context.Context, req engineRequest) error {
	conn, err := c.acquire(ctx)
	if err != nil {
		return err
	}

	payload, err := msgpack.Marshal(&req)
	if err != nil {
		c.release(conn, false)
		return fmt.Errorf("marshal: %w", err)
	}

	// 4-byte big-endian length prefix.
	var hdr [4]byte
	binary.BigEndian.PutUint32(hdr[:], uint32(len(payload)))

	deadline := time.Now().Add(c.reqTimeout)
	conn.SetDeadline(deadline) //nolint:errcheck

	if _, err := conn.Write(hdr[:]); err != nil {
		c.release(conn, false)
		return fmt.Errorf("write header: %w", err)
	}
	if _, err := conn.Write(payload); err != nil {
		c.release(conn, false)
		return fmt.Errorf("write payload: %w", err)
	}

	// Read response length.
	if _, err := io.ReadFull(conn, hdr[:]); err != nil {
		c.release(conn, false)
		return fmt.Errorf("read response header: %w", err)
	}
	respLen := binary.BigEndian.Uint32(hdr[:])
	buf := make([]byte, respLen)
	if _, err := io.ReadFull(conn, buf); err != nil {
		c.release(conn, false)
		return fmt.Errorf("read response: %w", err)
	}

	var resp engineResponse
	if err := msgpack.Unmarshal(buf, &resp); err != nil {
		c.release(conn, false)
		return fmt.Errorf("unmarshal response: %w", err)
	}
	c.release(conn, true)

	if !resp.OK {
		if resp.Error != nil {
			return fmt.Errorf("engine error: %s", *resp.Error)
		}
		return fmt.Errorf("engine: operation rejected")
	}
	return nil
}

// Debit implements EngineClient.
func (c *TcpEngineClient) Debit(ctx context.Context, accountID string, amount int64) error {
	return c.call(ctx, engineRequest{
		RequestID: fmt.Sprintf("debit-%s-%d", accountID, time.Now().UnixNano()),
		AccountID: accountID,
		Amount:    amount,
		Op:        "debit",
	})
}

// Credit implements EngineClient.
func (c *TcpEngineClient) Credit(ctx context.Context, accountID string, amount int64) error {
	return c.call(ctx, engineRequest{
		RequestID: fmt.Sprintf("credit-%s-%d", accountID, time.Now().UnixNano()),
		AccountID: accountID,
		Amount:    amount,
		Op:        "credit",
	})
}

// compile-time interface check.
var _ EngineClient = (*TcpEngineClient)(nil)

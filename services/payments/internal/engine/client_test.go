package engine_test

import (
	"context"
	"net"
	"testing"
	"time"

	"github.com/blazil/services/payments/internal/engine"
)

// startEchoServer starts a minimal TCP server that immediately closes each
// accepted connection. Used to test pool behaviour.
func startEchoServer(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to listen: %v", err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			// Echo server: close immediately (simulates broken conn scenario).
			conn.Close()
		}
	}()
	return ln.Addr().String()
}

// startAcceptingServer starts a TCP server that accepts connections and keeps
// them open (never closes them from the server side).
func startAcceptingServer(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to listen: %v", err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			// Keep connection open until server shuts down.
			go func() { _, _ = net.Dial("tcp", conn.LocalAddr().String()) }()
			_ = conn
		}
	}()
	return ln.Addr().String()
}

func TestConnectionPool_AcquireRelease(t *testing.T) {
	addr := startAcceptingServer(t)
	pool, err := engine.NewConnectionPool(addr, 3, 2*time.Second)
	if err != nil {
		t.Fatalf("NewConnectionPool: %v", err)
	}
	defer pool.Close()

	ctx := context.Background()

	// Acquire a connection.
	conn, err := pool.Acquire(ctx)
	if err != nil {
		t.Fatalf("acquire: %v", err)
	}
	if conn == nil {
		t.Fatal("expected non-nil connection")
	}

	// Release it back without error.
	pool.Release(conn, nil)

	// Acquire again — should succeed (connection was returned).
	conn2, err := pool.Acquire(ctx)
	if err != nil {
		t.Fatalf("second acquire: %v", err)
	}
	pool.Release(conn2, nil)
}

func TestConnectionPool_ReplacesDeadConnection(t *testing.T) {
	addr := startAcceptingServer(t)
	pool, err := engine.NewConnectionPool(addr, 2, 2*time.Second)
	if err != nil {
		t.Fatalf("NewConnectionPool: %v", err)
	}
	defer pool.Close()

	ctx := context.Background()

	conn, err := pool.Acquire(ctx)
	if err != nil {
		t.Fatalf("acquire: %v", err)
	}

	// Simulate a broken connection by releasing with an error.
	conn.Close()
	pool.Release(conn, net.ErrClosed)

	// Give background replacement goroutine time to dial.
	time.Sleep(100 * time.Millisecond)

	// Pool should still be able to provide a connection (replacement was dialled).
	ctx2, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	conn2, err := pool.Acquire(ctx2)
	if err != nil {
		t.Fatalf("acquire after dead-conn replacement: %v", err)
	}
	pool.Release(conn2, nil)
}

func TestConnectionPool_ContextCancellation(t *testing.T) {
	// Point the pool at a non-listening address so pre-dial fails, leaving the
	// pool empty. Then cancel the context before a connection becomes available.
	pool := engine.NewEmptyPool("127.0.0.1:19999", 5*time.Second)
	defer pool.Close()

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	_, err := pool.Acquire(ctx)
	if err == nil {
		t.Fatal("expected error from cancelled context, got nil")
	}
}

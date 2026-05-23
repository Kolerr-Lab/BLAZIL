package proxy

import (
	"context"
	"strings"
	"sync"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/keepalive"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// Route maps an incoming gRPC service name prefix to an upstream address.
type Route struct {
	// ServicePrefix is the dotted package + service prefix, e.g. "payments.v1".
	// Matched against the full method name: "/payments.v1.PaymentsService/Submit".
	ServicePrefix string
	// UpstreamAddr is the host:port of the upstream gRPC server.
	UpstreamAddr string
}

// Director maintains a connection pool to upstream gRPC services and routes
// incoming calls based on their full method name.
//
// Connections are created lazily on first use and reused for the lifetime of
// the Director. Use Close() during graceful shutdown to release them.
type Director struct {
	routes []Route

	mu    sync.RWMutex
	conns map[string]*grpc.ClientConn // keyed by UpstreamAddr

	dialOpts []grpc.DialOption
}

// NewDirector creates a Director using the given routes.
func NewDirector(routes []Route) *Director {
	return &Director{
		routes: routes,
		conns:  make(map[string]*grpc.ClientConn),
		dialOpts: []grpc.DialOption{
			// Internal east-west traffic uses plaintext; TLS is terminated at the
			// ingress load balancer. To enable mTLS, replace with credentials.NewTLS(...).
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			// All upstream connections share the raw proxy codec — no protobuf decoding.
			grpc.WithDefaultCallOptions(grpc.ForceCodec(Codec())),
			grpc.WithKeepaliveParams(keepalive.ClientParameters{
				Time:                30 * time.Second,
				Timeout:             5 * time.Second,
				PermitWithoutStream: true,
			}),
		},
	}
}

// Direct selects an upstream connection for the given full gRPC method name.
// The incoming metadata from ctx is forwarded to the upstream as-is.
//
// Returns codes.Unavailable if no route matches or the connection cannot be
// established. Returns codes.Unimplemented if the method prefix is unknown.
func (d *Director) Direct(ctx context.Context, fullMethod string) (context.Context, *grpc.ClientConn, error) {
	addr := d.routeFor(fullMethod)
	if addr == "" {
		return ctx, nil, status.Errorf(codes.Unimplemented,
			"gateway: no route configured for method %s", fullMethod)
	}

	conn, err := d.connFor(addr)
	if err != nil {
		return ctx, nil, status.Errorf(codes.Unavailable,
			"gateway: upstream %s unavailable: %v", addr, err)
	}

	// Forward all incoming metadata to the upstream service unchanged.
	// This preserves trace IDs, auth headers (already validated), and any
	// custom metadata the client sent.
	md, _ := metadata.FromIncomingContext(ctx)
	outCtx := metadata.NewOutgoingContext(ctx, md.Copy())
	return outCtx, conn, nil
}

// routeFor returns the upstream address for the given full gRPC method name.
// Routes are evaluated in order; the first matching prefix wins.
// Returns "" if no route matches.
func (d *Director) routeFor(fullMethod string) string {
	// fullMethod has the form "/package.ServiceName/MethodName".
	// Strip the leading "/" for prefix matching.
	m := strings.TrimPrefix(fullMethod, "/")
	for _, r := range d.routes {
		if strings.HasPrefix(m, r.ServicePrefix) {
			return r.UpstreamAddr
		}
	}
	return ""
}

// connFor returns an existing connection for addr or dials a new one.
// Connections are created at most once per address (double-checked locking).
func (d *Director) connFor(addr string) (*grpc.ClientConn, error) {
	d.mu.RLock()
	conn, ok := d.conns[addr]
	d.mu.RUnlock()
	if ok {
		return conn, nil
	}

	d.mu.Lock()
	defer d.mu.Unlock()
	// Re-check after acquiring write lock.
	if conn, ok = d.conns[addr]; ok {
		return conn, nil
	}

	var err error
	conn, err = grpc.NewClient(addr, d.dialOpts...)
	if err != nil {
		return nil, err
	}
	d.conns[addr] = conn
	return conn, nil
}

// Close releases all upstream connections.
// Call during graceful shutdown after the gRPC server has stopped accepting
// new connections.
func (d *Director) Close() {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, conn := range d.conns {
		_ = conn.Close()
	}
	d.conns = make(map[string]*grpc.ClientConn)
}

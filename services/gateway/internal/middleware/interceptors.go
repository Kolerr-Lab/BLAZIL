// Package middleware provides gRPC stream server interceptors for the gateway.
//
// Interceptor chain order (outermost to innermost):
//  1. AuthStreamInterceptor   — validates API key, injects Tenant into context
//  2. RateLimitStreamInterceptor — token-bucket check per tenant
//  3. MeteringStreamInterceptor  — records +1/transaction after successful proxy
//
// Interceptors are composed with grpc.ChainStreamInterceptor in main.go.
package middleware

import (
	"context"

	"github.com/blazil/metering"
	"github.com/blazil/services/gateway/internal/ratelimit"
	"github.com/blazil/services/gateway/internal/tenant"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// contextKey is an unexported type to prevent collisions with keys from other
// packages in the context value space.
type contextKey string

const tenantContextKey contextKey = "tenant"

// TenantFromContext retrieves the authenticated Tenant from a gRPC stream
// context. Returns nil if not set (should not happen after AuthStreamInterceptor).
func TenantFromContext(ctx context.Context) *tenant.Tenant {
	v := ctx.Value(tenantContextKey)
	if v == nil {
		return nil
	}
	t, _ := v.(*tenant.Tenant)
	return t
}

// AuthStreamInterceptor validates the "x-blazil-key" gRPC metadata header and
// populates the stream context with the authenticated Tenant.
//
// On failure it returns:
//   - codes.Unauthenticated — missing, invalid, or revoked key
//   - codes.PermissionDenied — tenant suspended
func AuthStreamInterceptor(store tenant.Store) grpc.StreamServerInterceptor {
	return func(
		srv interface{},
		ss grpc.ServerStream,
		_ *grpc.StreamServerInfo,
		handler grpc.StreamHandler,
	) error {
		md, ok := metadata.FromIncomingContext(ss.Context())
		if !ok {
			return status.Error(codes.Unauthenticated, "missing metadata")
		}

		values := md.Get("x-blazil-key")
		if len(values) == 0 || values[0] == "" {
			return status.Error(codes.Unauthenticated, "missing x-blazil-key header")
		}
		rawKey := values[0]

		t, _, err := store.LookupAPIKey(ss.Context(), rawKey)
		if err != nil {
			switch err {
			case tenant.ErrNotFound, tenant.ErrKeyRevoked:
				return status.Error(codes.Unauthenticated, "invalid or revoked api key")
			case tenant.ErrSuspended:
				return status.Error(codes.PermissionDenied, "tenant account suspended")
			default:
				return status.Error(codes.Internal, "authentication error")
			}
		}

		// Inject the authenticated tenant into the stream context.
		wrapped := &wrappedServerStream{
			ServerStream: ss,
			ctx:          context.WithValue(ss.Context(), tenantContextKey, t),
		}
		return handler(srv, wrapped)
	}
}

// RateLimitStreamInterceptor enforces per-tenant token-bucket rate limits.
// Must be placed after AuthStreamInterceptor so the Tenant is available.
func RateLimitStreamInterceptor(limiter *ratelimit.Limiter) grpc.StreamServerInterceptor {
	return func(
		srv interface{},
		ss grpc.ServerStream,
		_ *grpc.StreamServerInfo,
		handler grpc.StreamHandler,
	) error {
		t := TenantFromContext(ss.Context())
		if t == nil {
			// Should never happen if auth interceptor ran first.
			return status.Error(codes.Internal, "tenant missing from context")
		}
		if !limiter.Allow(t.ID, t.RateLimitRPS, t.RateLimitBurst) {
			return status.Errorf(codes.ResourceExhausted,
				"rate limit exceeded: %s", t.ID)
		}
		return handler(srv, ss)
	}
}

// MeteringStreamInterceptor records one transaction unit per successfully
// completed proxied stream. Must be placed after AuthStreamInterceptor.
func MeteringStreamInterceptor(rec metering.Recorder) grpc.StreamServerInterceptor {
	return func(
		srv interface{},
		ss grpc.ServerStream,
		_ *grpc.StreamServerInfo,
		handler grpc.StreamHandler,
	) error {
		t := TenantFromContext(ss.Context())
		if t == nil {
			return status.Error(codes.Internal, "tenant missing from context")
		}
		err := handler(srv, ss)
		if err == nil {
			// Record usage only on clean success to avoid counting failed/aborted calls.
			rec.Record(t.ID, 1)
		}
		return err
	}
}

// wrappedServerStream wraps a grpc.ServerStream to override its context.
type wrappedServerStream struct {
	grpc.ServerStream
	ctx context.Context
}

func (w *wrappedServerStream) Context() context.Context {
	return w.ctx
}

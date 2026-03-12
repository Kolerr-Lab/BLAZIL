package auth

import (
	"context"
	"os"
	"strings"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// AuthInterceptor returns a gRPC UnaryServerInterceptor that validates Bearer tokens.
//
// Behaviour:
//   - Extracts "authorization: Bearer <token>" from gRPC metadata.
//   - Calls validator.ValidateToken(); on failure returns codes.Unauthenticated.
//   - On success: embeds Claims into the context via contextWithClaims.
//   - If token is absent AND BLAZIL_AUTH_REQUIRED=false: passes through (dev mode).
//
// Security: the raw token string is never logged.
func AuthInterceptor(validator TokenValidator) grpc.UnaryServerInterceptor {
	return func(
		ctx context.Context,
		req interface{},
		info *grpc.UnaryServerInfo,
		handler grpc.UnaryHandler,
	) (interface{}, error) {
		token := extractBearerToken(ctx)

		if token == "" {
			// Dev mode bypass: allow unauthenticated calls when auth not required.
			if !authRequired() {
				return handler(ctx, req)
			}
			return nil, status.Error(codes.Unauthenticated, "missing authorization token")
		}

		claims, err := validator.ValidateToken(ctx, token)
		if err != nil {
			return nil, status.Errorf(codes.Unauthenticated, "invalid token")
		}

		return handler(contextWithClaims(ctx, claims), req)
	}
}

// extractBearerToken reads the "authorization" metadata key and strips the "Bearer " prefix.
// Returns empty string if not present or not a Bearer token.
func extractBearerToken(ctx context.Context) string {
	md, ok := metadata.FromIncomingContext(ctx)
	if !ok {
		return ""
	}
	vals := md.Get("authorization")
	if len(vals) == 0 {
		return ""
	}
	v := vals[0]
	const prefix = "Bearer "
	if !strings.HasPrefix(v, prefix) {
		return ""
	}
	return strings.TrimPrefix(v, prefix)
}

// authRequired returns true when BLAZIL_AUTH_REQUIRED != "false".
// Defaults to true in production; set to "false" in dev/demo.
func authRequired() bool {
	return os.Getenv("BLAZIL_AUTH_REQUIRED") != "false"
}

// Package auth provides JWT token validation for Blazil gRPC services.
//
// Security: Token values are never logged. Only Claims.Subject and Roles
// are accessible after validation; raw JWT strings are discarded.
//
// Dev mode: If KEYCLOAK_URL is empty, NewJWTValidator returns a
// MockTokenValidator that accepts all tokens (allows running without Keycloak).
package auth

import (
	"context"
	"time"
)

// Claims holds the parsed, validated JWT claims.
type Claims struct {
	Subject   string    // user ID from JWT "sub" field
	Roles     []string  // ["payment:write", "trading:read", ...]
	ExpiresAt time.Time
	Issuer    string
}

// HasRole returns true if the claims include the given role.
func (c *Claims) HasRole(role string) bool {
	for _, r := range c.Roles {
		if r == role {
			return true
		}
	}
	return false
}

// TokenValidator is the interface for validating Bearer tokens.
type TokenValidator interface {
	ValidateToken(ctx context.Context, token string) (*Claims, error)
}

// contextKey is an unexported type for context keys in this package.
type contextKey struct{ name string }

// claimsKey is the context key for storing Claims.
var claimsKey = &contextKey{"claims"}

// ClaimsFromContext extracts Claims previously stored by AuthInterceptor.
// Returns nil if no claims are present.
func ClaimsFromContext(ctx context.Context) *Claims {
	c, _ := ctx.Value(claimsKey).(*Claims)
	return c
}

// contextWithClaims returns a new context with Claims embedded.
func contextWithClaims(ctx context.Context, c *Claims) context.Context {
	return context.WithValue(ctx, claimsKey, c)
}

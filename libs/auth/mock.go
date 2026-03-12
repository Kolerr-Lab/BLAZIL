package auth

import (
	"context"
	"time"
)

// MockTokenValidator accepts or rejects tokens based on a simple in-memory registry.
// The zero value (via NewMockTokenValidator) accepts all non-empty tokens.
type MockTokenValidator struct {
	// validTokens maps token string → Claims. If nil, all non-empty tokens are accepted
	// with a default admin Claims.
	validTokens map[string]*Claims
}

// NewMockTokenValidator returns a validator that accepts all non-empty tokens
// with default admin claims. Used in dev/demo mode when KEYCLOAK_URL is empty.
func NewMockTokenValidator() *MockTokenValidator {
	return &MockTokenValidator{}
}

// NewMockValidatorWithTokens creates a mock with specific token→claims mappings.
// Tokens not in the map are rejected. Useful for fine-grained auth tests.
func NewMockValidatorWithTokens(tokens map[string]*Claims) *MockTokenValidator {
	return &MockTokenValidator{validTokens: tokens}
}

func (m *MockTokenValidator) ValidateToken(_ context.Context, token string) (*Claims, error) {
	if token == "" {
		return nil, ErrUnauthenticated
	}
	if m.validTokens != nil {
		claims, ok := m.validTokens[token]
		if !ok {
			return nil, ErrTokenInvalid
		}
		if time.Now().After(claims.ExpiresAt) {
			return nil, ErrTokenExpired
		}
		return claims, nil
	}
	// Default: accept any non-empty token with admin claims.
	return &Claims{
		Subject:   "dev-user",
		Roles:     []string{"admin", "payment:write", "trading:write", "balance:read"},
		ExpiresAt: time.Now().Add(24 * time.Hour),
		Issuer:    "blazil-dev",
	}, nil
}

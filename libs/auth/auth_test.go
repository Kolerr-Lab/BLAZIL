package auth_test

import (
	"context"
	"testing"
	"time"

	"github.com/blazil/auth"
	"google.golang.org/grpc"
	"google.golang.org/grpc/metadata"
)

// TestMockValidator_ValidToken verifies that a seeded valid token returns correct Claims.
func TestMockValidator_ValidToken(t *testing.T) {
	token := "valid-test-token"
	claims := &auth.Claims{
		Subject:   "user-123",
		Roles:     []string{"payment:write", "trading:read"},
		ExpiresAt: time.Now().Add(time.Hour),
		Issuer:    "https://keycloak.example.com/realms/blazil",
	}
	v := auth.NewMockValidatorWithTokens(map[string]*auth.Claims{token: claims})

	got, err := v.ValidateToken(context.Background(), token)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got.Subject != "user-123" {
		t.Errorf("subject: got %q, want %q", got.Subject, "user-123")
	}
	if !got.HasRole("payment:write") {
		t.Error("expected payment:write role")
	}
}

// TestMockValidator_ExpiredToken_Error verifies that an expired token returns ErrTokenExpired.
func TestMockValidator_ExpiredToken_Error(t *testing.T) {
	token := "expired-token"
	claims := &auth.Claims{
		Subject:   "user-456",
		Roles:     []string{"payment:write"},
		ExpiresAt: time.Now().Add(-time.Hour), // expired
		Issuer:    "blazil",
	}
	v := auth.NewMockValidatorWithTokens(map[string]*auth.Claims{token: claims})

	_, err := v.ValidateToken(context.Background(), token)
	if err == nil {
		t.Fatal("expected error for expired token, got nil")
	}
	if err != auth.ErrTokenExpired {
		t.Errorf("expected ErrTokenExpired, got %v", err)
	}
}

// TestMockValidator_InvalidRole verifies that HasRole returns false for absent roles.
func TestMockValidator_InvalidRole(t *testing.T) {
	v := auth.NewMockTokenValidator()
	claims, err := v.ValidateToken(context.Background(), "any-token")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if claims.HasRole("nonexistent:role") {
		t.Error("expected HasRole to return false for nonexistent role")
	}
}

// TestAuthInterceptor_NoToken_DevMode_Passes verifies that missing token passes
// when BLAZIL_AUTH_REQUIRED=false.
func TestAuthInterceptor_NoToken_DevMode_Passes(t *testing.T) {
	t.Setenv("BLAZIL_AUTH_REQUIRED", "false")

	v := auth.NewMockTokenValidator()
	interceptor := auth.AuthInterceptor(v)

	called := false
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		called = true
		return "ok", nil
	}

	ctx := context.Background() // no metadata → no token
	resp, err := interceptor(ctx, nil, &grpc.UnaryServerInfo{FullMethod: "/test.v1/Test"}, handler)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp != "ok" {
		t.Errorf("expected ok response, got %v", resp)
	}
	if !called {
		t.Error("handler was not called")
	}
}

// TestAuthInterceptor_ValidToken_AddsClaims verifies that a valid Bearer token
// results in Claims being embedded in the context.
func TestAuthInterceptor_ValidToken_AddsClaims(t *testing.T) {
	t.Setenv("BLAZIL_AUTH_REQUIRED", "true")

	token := "bearer-test-token"
	expected := &auth.Claims{
		Subject:   "user-789",
		Roles:     []string{"admin"},
		ExpiresAt: time.Now().Add(time.Hour),
		Issuer:    "blazil",
	}
	v := auth.NewMockValidatorWithTokens(map[string]*auth.Claims{token: expected})
	interceptor := auth.AuthInterceptor(v)

	var gotClaims *auth.Claims
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		gotClaims = auth.ClaimsFromContext(ctx)
		return "ok", nil
	}

	md := metadata.Pairs("authorization", "Bearer "+token)
	ctx := metadata.NewIncomingContext(context.Background(), md)
	_, err := interceptor(ctx, nil, &grpc.UnaryServerInfo{FullMethod: "/test.v1/Test"}, handler)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotClaims == nil {
		t.Fatal("expected Claims in context, got nil")
	}
	if gotClaims.Subject != "user-789" {
		t.Errorf("subject: got %q, want %q", gotClaims.Subject, "user-789")
	}
}

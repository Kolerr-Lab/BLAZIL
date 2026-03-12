package secrets_test

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/blazil/secrets"
)

// TestMockVault_GetSecret verifies that seeded values are returned correctly.
func TestMockVault_GetSecret(t *testing.T) {
	m := secrets.NewMockSecretClient()
	m.Seed("secret/data/payments", "engine_conn_string", "127.0.0.1:7878")

	kv, err := m.GetSecret(context.Background(), "secret/data/payments")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if kv["engine_conn_string"] != "127.0.0.1:7878" {
		t.Errorf("got %q, want %q", kv["engine_conn_string"], "127.0.0.1:7878")
	}
}

// TestMockVault_PutSecret verifies that PutSecret stores and GetSecret retrieves.
func TestMockVault_PutSecret(t *testing.T) {
	m := secrets.NewMockSecretClient()
	ctx := context.Background()

	if err := m.PutSecret(ctx, "secret/data/banking", "api_key", "test-key-123"); err != nil {
		t.Fatalf("PutSecret error: %v", err)
	}
	kv, err := m.GetSecret(ctx, "secret/data/banking")
	if err != nil {
		t.Fatalf("GetSecret error: %v", err)
	}
	if kv["api_key"] != "test-key-123" {
		t.Errorf("got %q, want %q", kv["api_key"], "test-key-123")
	}
}

// TestMockVault_MissingKey_Error verifies that missing paths return ErrSecretNotFound.
func TestMockVault_MissingKey_Error(t *testing.T) {
	m := secrets.NewMockSecretClient()
	_, err := m.GetSecret(context.Background(), "secret/data/does-not-exist")
	if err == nil {
		t.Fatal("expected error for missing path, got nil")
	}
	if err != secrets.ErrSecretNotFound {
		t.Errorf("expected ErrSecretNotFound, got %v", err)
	}
}

// TestVaultClient_FallbackToEnv verifies that when Vault is unreachable,
// LoadOrEnv returns the value from the environment variable.
func TestVaultClient_FallbackToEnv(t *testing.T) {
	// Start an HTTP server that always returns 503.
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Return a valid but empty JSON so it's clearly a Vault error, not parse error
		w.WriteHeader(http.StatusServiceUnavailable)
		json.NewEncoder(w).Encode(map[string]string{"errors": ""})
	}))
	defer srv.Close()

	const envKey = "TEST_BLAZIL_SECRET_CONN"
	os.Setenv(envKey, "fallback-engine-addr:7878")
	defer os.Unsetenv(envKey)

	client := secrets.NewVaultClientWithAddr(srv.URL, "")
	result := secrets.LoadOrEnv(context.Background(), client, "secret/data/payments", "engine_conn_string", envKey, "default-value")

	if result != "fallback-engine-addr:7878" {
		t.Errorf("expected env fallback, got %q", result)
	}
}

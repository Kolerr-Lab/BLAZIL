package tenant_test

import (
	"strings"
	"testing"

	"github.com/blazil/services/gateway/internal/tenant"
)

func TestGenerateAPIKey_Format(t *testing.T) {
	raw, hash, prefix, err := tenant.GenerateAPIKey()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Raw key must start with the live prefix.
	if !strings.HasPrefix(raw, "blz_live_") {
		t.Errorf("raw key missing prefix: %q", raw[:min(20, len(raw))])
	}

	// Raw key = "blz_live_" (9) + 64 hex chars (32 bytes) = 73 chars.
	const wantLen = 9 + 64
	if len(raw) != wantLen {
		t.Errorf("raw key length: want %d, got %d", wantLen, len(raw))
	}

	// Hash must be 64 hex chars (SHA-256 = 32 bytes).
	if len(hash) != 64 {
		t.Errorf("hash length: want 64, got %d", len(hash))
	}

	// Prefix is the first 16 chars.
	if prefix != raw[:16] {
		t.Errorf("prefix mismatch: want %q, got %q", raw[:16], prefix)
	}
}

func TestGenerateAPIKey_UniquePerCall(t *testing.T) {
	raw1, hash1, _, _ := tenant.GenerateAPIKey()
	raw2, hash2, _, _ := tenant.GenerateAPIKey()

	if raw1 == raw2 {
		t.Error("two calls returned the same raw key")
	}
	if hash1 == hash2 {
		t.Error("two calls returned the same hash")
	}
}

func TestHashAPIKey_Deterministic(t *testing.T) {
	const key = "blz_live_testkey"
	h1 := tenant.HashAPIKey(key)
	h2 := tenant.HashAPIKey(key)
	if h1 != h2 {
		t.Errorf("hash is not deterministic: %q vs %q", h1, h2)
	}
	// Must be lowercase hex.
	for _, c := range h1 {
		if !strings.ContainsRune("0123456789abcdef", c) {
			t.Errorf("hash contains non-hex character: %c", c)
		}
	}
}

func TestHashAPIKey_DifferentKeysProduceDifferentHashes(t *testing.T) {
	if tenant.HashAPIKey("key-a") == tenant.HashAPIKey("key-b") {
		t.Error("different keys produced the same hash")
	}
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

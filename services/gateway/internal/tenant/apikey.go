package tenant

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
)

const (
	// keyEntropyBytes is the number of random bytes in each API key (256-bit entropy).
	keyEntropyBytes = 32

	// keyPrefix is prepended to all live production keys.
	// Prefixed keys are easily grepped in logs without accidentally leaking value.
	keyPrefix = "blz_live_"

	// KeyPrefixDisplayLen is how many leading characters of the raw key are
	// safe to surface in dashboards (the prefix + 7 random hex chars = "blz_live_XXXXXXX").
	KeyPrefixDisplayLen = 16
)

// GenerateAPIKey generates a cryptographically secure API key.
//
// Returns:
//   - rawKey:  the full key string (e.g. "blz_live_a3f..."). Return to the caller
//     exactly once; never store it.
//   - keyHash: lowercase hex-encoded SHA-256(rawKey). Store this in the DB.
//   - prefix:  the first KeyPrefixDisplayLen characters of rawKey; safe for UI display.
func GenerateAPIKey() (rawKey, keyHash, prefix string, err error) {
	b := make([]byte, keyEntropyBytes)
	if _, err = rand.Read(b); err != nil {
		return "", "", "", fmt.Errorf("generate api key: %w", err)
	}
	rawKey = keyPrefix + hex.EncodeToString(b)
	keyHash = HashAPIKey(rawKey)
	prefix = rawKey[:min(KeyPrefixDisplayLen, len(rawKey))]
	return rawKey, keyHash, prefix, nil
}

// HashAPIKey returns the SHA-256 hex digest of the given raw key.
// This is the value stored in the database and used for O(1) lookup.
func HashAPIKey(rawKey string) string {
	h := sha256.Sum256([]byte(rawKey))
	return hex.EncodeToString(h[:])
}

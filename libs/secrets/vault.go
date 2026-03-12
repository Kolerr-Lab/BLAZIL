// Package secrets provides Vault-backed secret management with env-var fallback.
//
// Security: No secrets are logged. Tokens are read from env only.
// Dev mode: If VAULT_ADDR is unset, all GetSecret calls return ErrUnavailable
// and callers fall back to env vars transparently.
package secrets

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

// ErrUnavailable is returned when Vault is not reachable.
var ErrUnavailable = errors.New("vault unavailable")

// ErrSecretNotFound is returned when the key does not exist at the given path.
var ErrSecretNotFound = errors.New("secret not found")

// VaultClient defines the interface for secret management.
type VaultClient interface {
	// GetSecret retrieves all key/value pairs at path.
	GetSecret(ctx context.Context, path string) (map[string]string, error)
	// PutSecret stores a key/value pair at path.
	PutSecret(ctx context.Context, path, key, value string) error
	// RotateSecret regenerates the secret at path (stub — signals external rotation).
	RotateSecret(ctx context.Context, path string) error
}

// VaultClientImpl calls the Vault KV v2 HTTP API.
// Reads VAULT_ADDR and VAULT_TOKEN from env on construction.
// All HTTP calls have a 5-second timeout.
type VaultClientImpl struct {
	addr   string
	token  string
	client *http.Client
}

// NewVaultClientImpl creates a new Vault client from env vars.
// If VAULT_ADDR is empty, GetSecret returns ErrUnavailable (safe for dev mode).
func NewVaultClientImpl() *VaultClientImpl {
	return &VaultClientImpl{
		addr:  getEnv("VAULT_ADDR", ""),
		token: os.Getenv("VAULT_TOKEN"),
		client: &http.Client{
			Timeout: 5 * time.Second,
		},
	}
}

// NewVaultClientWithAddr creates a client pointing at a specific address.
// Primarily for tests.
func NewVaultClientWithAddr(addr, token string) *VaultClientImpl {
	return &VaultClientImpl{
		addr:  addr,
		token: token,
		client: &http.Client{
			Timeout: 5 * time.Second,
		},
	}
}

func (v *VaultClientImpl) GetSecret(ctx context.Context, path string) (map[string]string, error) {
	if v.addr == "" {
		return nil, ErrUnavailable
	}
	url := fmt.Sprintf("%s/v1/%s", strings.TrimRight(v.addr, "/"), path)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, fmt.Errorf("build vault request: %w", err)
	}
	if v.token != "" {
		req.Header.Set("X-Vault-Token", v.token)
	}

	resp, err := v.client.Do(req)
	if err != nil {
		return nil, ErrUnavailable
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, ErrSecretNotFound
	}
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return nil, fmt.Errorf("vault returned %d: %s", resp.StatusCode, body)
	}

	// KV v2 response: {"data": {"data": {"key": "value"}}}
	var payload struct {
		Data struct {
			Data map[string]string `json:"data"`
		} `json:"data"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return nil, fmt.Errorf("decode vault response: %w", err)
	}
	return payload.Data.Data, nil
}

func (v *VaultClientImpl) PutSecret(ctx context.Context, path, key, value string) error {
	if v.addr == "" {
		return ErrUnavailable
	}
	url := fmt.Sprintf("%s/v1/%s", strings.TrimRight(v.addr, "/"), path)
	body := fmt.Sprintf(`{"data":{%q:%q}}`, key, value)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, strings.NewReader(body))
	if err != nil {
		return fmt.Errorf("build vault request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	if v.token != "" {
		req.Header.Set("X-Vault-Token", v.token)
	}

	resp, err := v.client.Do(req)
	if err != nil {
		return ErrUnavailable
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusNoContent {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return fmt.Errorf("vault put returned %d: %s", resp.StatusCode, b)
	}
	return nil
}

func (v *VaultClientImpl) RotateSecret(ctx context.Context, path string) error {
	if v.addr == "" {
		return ErrUnavailable
	}
	// Stub: signals an external rotation policy. Real implementation would
	// call a Vault plugin or external rotation tool.
	return nil
}

// LoadOrEnv tries to get key from Vault at path; falls back to envVar default.
// Never returns an error — Vault failures are silenced and env var is used.
// Security: value is never logged.
func LoadOrEnv(ctx context.Context, client VaultClient, path, key, envVar, defaultVal string) string {
	secrets, err := client.GetSecret(ctx, path)
	if err == nil {
		if val, ok := secrets[key]; ok && val != "" {
			return val
		}
	}
	if v := os.Getenv(envVar); v != "" {
		return v
	}
	return defaultVal
}

func getEnv(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

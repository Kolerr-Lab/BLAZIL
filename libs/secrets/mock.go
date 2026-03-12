package secrets

import (
	"context"
	"fmt"
)

// MockSecretClient is an in-memory VaultClient for tests.
// Thread-safe for concurrent reads; not intended for concurrent PutSecret + GetSecret.
type MockSecretClient struct {
	store map[string]map[string]string
}

// NewMockSecretClient returns an empty mock client.
func NewMockSecretClient() *MockSecretClient {
	return &MockSecretClient{
		store: make(map[string]map[string]string),
	}
}

// Seed sets path/key/value directly — useful for test setup.
func (m *MockSecretClient) Seed(path, key, value string) {
	if _, ok := m.store[path]; !ok {
		m.store[path] = make(map[string]string)
	}
	m.store[path][key] = value
}

func (m *MockSecretClient) GetSecret(_ context.Context, path string) (map[string]string, error) {
	kv, ok := m.store[path]
	if !ok {
		return nil, ErrSecretNotFound
	}
	// Return a copy to prevent external mutation.
	out := make(map[string]string, len(kv))
	for k, v := range kv {
		out[k] = v
	}
	return out, nil
}

func (m *MockSecretClient) PutSecret(_ context.Context, path, key, value string) error {
	if _, ok := m.store[path]; !ok {
		m.store[path] = make(map[string]string)
	}
	m.store[path][key] = value
	return nil
}

func (m *MockSecretClient) RotateSecret(_ context.Context, path string) error {
	if _, ok := m.store[path]; !ok {
		return fmt.Errorf("%w: path %q not found", ErrSecretNotFound, path)
	}
	// Mock rotation: appends "-rotated" to all values.
	for k, v := range m.store[path] {
		m.store[path][k] = v + "-rotated"
	}
	return nil
}

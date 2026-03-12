// Package config loads banking service configuration from environment variables.
package config

import (
	"context"
	"os"
	"time"

	"github.com/blazil/secrets"
)

// Config holds all runtime configuration for the banking service.
type Config struct {
	// GRPCAddr is the address the gRPC server listens on. Default: ":50052".
	GRPCAddr string

	// EngineAddr is the TCP address of the Blazil Rust transport server.
	// Default: "127.0.0.1:7878"
	EngineAddr string

	// LogLevel controls the minimum log severity. Default: "info".
	LogLevel string

	// IdempotencyTTL is unused in this iteration but reserved for future use.
	IdempotencyTTL time.Duration

	// MetricsAddr is the address the Prometheus metrics HTTP server listens on.
	// Default: ":9092"
	MetricsAddr string
}

// Load reads configuration from environment variables with defaults.
// Vault is consulted first for EngineAddr; falls back silently.
func Load() Config {
	vc := secrets.NewVaultClientImpl()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	return Config{
		GRPCAddr:       getEnv("BANKING_GRPC_ADDR", ":50052"),
		EngineAddr:     secrets.LoadOrEnv(ctx, vc, "secret/data/banking", "engine_conn_string", "BLAZIL_ENGINE_ADDR", "127.0.0.1:7878"),
		LogLevel:       getEnv("LOG_LEVEL", "info"),
		IdempotencyTTL: 24 * time.Hour,
		MetricsAddr:    getEnv("BANKING_METRICS_ADDR", ":9092"),
	}
}

func getEnv(key, fallback string) string {
	if v, ok := os.LookupEnv(key); ok && v != "" {
		return v
	}
	return fallback
}

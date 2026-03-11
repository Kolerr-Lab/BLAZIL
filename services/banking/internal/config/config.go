// Package config loads banking service configuration from environment variables.
package config

import (
	"os"
	"time"
)

// Config holds all runtime configuration for the banking service.
type Config struct {
	// GRPCAddr is the address the gRPC server listens on. Default: ":50052".
	GRPCAddr string

	// LogLevel controls the minimum log severity. Default: "info".
	LogLevel string

	// IdempotencyTTL is unused in this iteration but reserved for future use.
	IdempotencyTTL time.Duration
}

// Load reads configuration from environment variables, falling back to defaults.
func Load() Config {
	return Config{
		GRPCAddr:       getEnv("BANKING_GRPC_ADDR", ":50052"),
		LogLevel:       getEnv("LOG_LEVEL", "info"),
		IdempotencyTTL: 24 * time.Hour,
	}
}

func getEnv(key, fallback string) string {
	if v, ok := os.LookupEnv(key); ok && v != "" {
		return v
	}
	return fallback
}

// Package config provides service configuration loaded from environment variables.
package config

import (
	"os"
	"strconv"
	"time"
)

// Config holds all runtime configuration for the payments service.
type Config struct {
	// GRPCAddr is the address the gRPC server listens on.
	// Default: ":50051"
	GRPCAddr string

	// EngineAddr is the TCP address of the Blazil Rust transport server.
	// Default: "127.0.0.1:7878"
	EngineAddr string

	// EngineTimeout is the per-request timeout for engine submissions.
	// Default: 5s
	EngineTimeout time.Duration

	// MaxAmountMinorUnits is the single-payment authorization limit in minor units.
	// Default: 100_000_000_00 ($1,000,000.00 USD equivalent)
	MaxAmountMinorUnits int64

	// IdempotencyTTL is how long idempotency keys are retained.
	// Default: 24h
	IdempotencyTTL time.Duration

	// LogLevel controls the structured log verbosity. Default: "info"
	LogLevel string

	// MetricsAddr is the address the Prometheus metrics HTTP server listens on.
	// Default: ":9091"
	MetricsAddr string
}

// Load reads configuration from environment variables, falling back to
// defaults for any unset variable.
func Load() Config {
	return Config{
		GRPCAddr:            envString("BLAZIL_GRPC_ADDR", ":50051"),
		EngineAddr:          envString("BLAZIL_ENGINE_ADDR", "127.0.0.1:7878"),
		EngineTimeout:       envDuration("BLAZIL_ENGINE_TIMEOUT", 5*time.Second),
		MaxAmountMinorUnits: envInt64("BLAZIL_MAX_AMOUNT_MINOR_UNITS", 100_000_000_00),
		IdempotencyTTL:      envDuration("BLAZIL_IDEMPOTENCY_TTL", 24*time.Hour),
		LogLevel:            envString("BLAZIL_LOG_LEVEL", "info"),
		MetricsAddr:         envString("BLAZIL_METRICS_ADDR", ":9091"),
	}
}

func envString(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func envInt64(key string, fallback int64) int64 {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil {
			return n
		}
	}
	return fallback
}

func envDuration(key string, fallback time.Duration) time.Duration {
	if v := os.Getenv(key); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			return d
		}
	}
	return fallback
}

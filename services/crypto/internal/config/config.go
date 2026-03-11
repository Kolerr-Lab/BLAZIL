// Package config loads the crypto service configuration from environment variables.
package config

import "os"

// Config holds the crypto service runtime configuration.
type Config struct {
	GRPCAddr string
	LogLevel string
}

// Load reads configuration from environment variables with defaults.
func Load() Config {
	grpcAddr := os.Getenv("GRPC_ADDR")
	if grpcAddr == "" {
		grpcAddr = ":50054"
	}
	logLevel := os.Getenv("LOG_LEVEL")
	if logLevel == "" {
		logLevel = "production"
	}
	return Config{
		GRPCAddr: grpcAddr,
		LogLevel: logLevel,
	}
}

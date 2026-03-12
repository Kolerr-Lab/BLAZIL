// Package config loads the trading service configuration from environment variables.
package config

import "os"

// Config holds the trading service runtime configuration.
type Config struct {
        GRPCAddr    string
        LogLevel    string
        MetricsAddr string
}

// Load reads configuration from environment variables with defaults.
func Load() Config {
        grpcAddr := os.Getenv("GRPC_ADDR")
        if grpcAddr == "" {
                grpcAddr = ":50053"
        }
        logLevel := os.Getenv("LOG_LEVEL")
        if logLevel == "" {
                logLevel = "production"
        }
        metricsAddr := os.Getenv("METRICS_ADDR")
        if metricsAddr == "" {
                metricsAddr = ":9093"
        }
        return Config{
                GRPCAddr:    grpcAddr,
                LogLevel:    logLevel,
                MetricsAddr: metricsAddr,
        }
}

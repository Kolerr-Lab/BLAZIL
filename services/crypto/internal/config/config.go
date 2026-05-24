// Package config loads the crypto service configuration from environment variables.
package config

import (
	"context"
	"os"
	"time"

	"github.com/blazil/secrets"
)

// Config holds the crypto service runtime configuration.
type Config struct {
	GRPCAddr    string
	EngineAddr  string
	DatabaseURL string
	EthNodeURL  string
	BtcNodeURL  string
	BtcRPCUser  string
	BtcRPCPass  string
	LogLevel    string
	MetricsAddr string
}

// Load reads configuration from environment variables with defaults.
// Vault is consulted first for EngineAddr; falls back silently.
func Load() Config {
	vc := secrets.NewVaultClientImpl()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	grpcAddr := os.Getenv("GRPC_ADDR")
	if grpcAddr == "" {
		grpcAddr = ":50054"
	}
	logLevel := os.Getenv("LOG_LEVEL")
	if logLevel == "" {
		logLevel = "production"
	}
	metricsAddr := os.Getenv("METRICS_ADDR")
	if metricsAddr == "" {
		metricsAddr = ":9094"
	}
	return Config{
		GRPCAddr:    grpcAddr,
		EngineAddr:  secrets.LoadOrEnv(ctx, vc, "secret/data/crypto", "engine_conn_string", "BLAZIL_ENGINE_ADDR", "127.0.0.1:7878"),
		DatabaseURL: secrets.LoadOrEnv(ctx, vc, "secret/data/crypto", "database_url", "CRYPTO_DATABASE_URL", ""),
		EthNodeURL:  secrets.LoadOrEnv(ctx, vc, "secret/data/crypto", "eth_node_url", "ETH_NODE_URL", ""),
		BtcNodeURL:  secrets.LoadOrEnv(ctx, vc, "secret/data/crypto", "btc_node_url", "BTC_NODE_URL", ""),
		BtcRPCUser:  os.Getenv("BTC_RPC_USER"),
		BtcRPCPass:  os.Getenv("BTC_RPC_PASS"),
		LogLevel:    logLevel,
		MetricsAddr: metricsAddr,
	}
}

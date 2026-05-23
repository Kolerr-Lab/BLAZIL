// Package config provides gateway service configuration from environment variables,
// with optional Vault lookup for sensitive values.
package config

import (
	"context"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/blazil/secrets"
)

// Config holds all runtime configuration for the Blazil API Gateway.
type Config struct {
	// GRPCAddr is the address the gRPC proxy server listens on (customer-facing).
	// Default: ":50050"
	GRPCAddr string

	// AdminAddr is the address the admin REST HTTP server listens on.
	// MUST be firewalled to internal networks only.
	// Default: ":8080"
	AdminAddr string

	// MetricsAddr is the Prometheus scrape endpoint (internal).
	// Default: ":9090"
	MetricsAddr string

	// DatabaseURL is the Postgres connection string.
	// Loaded from Vault secret/data/gateway key "database_url", or GATEWAY_DATABASE_URL.
	DatabaseURL string

	// AdminToken is the static bearer token required for admin API access.
	// Loaded from Vault secret/data/gateway key "admin_token", or GATEWAY_ADMIN_TOKEN.
	// If empty the admin API returns 503 — forces explicit configuration.
	AdminToken string

	// Routes maps gRPC service name prefixes to upstream host:port addresses.
	// Configured via GATEWAY_ROUTES as "prefix=addr,prefix=addr".
	// Default routes built from individual service address env vars.
	Routes []Route

	// LogLevel controls structured log verbosity. Default: "info"
	LogLevel string

	// ShutdownTimeout is how long the server waits for in-flight requests on SIGTERM.
	// Default: 30s
	ShutdownTimeout time.Duration

	// MaxRecvMsgSizeBytes is the maximum gRPC message size the gateway accepts.
	// Default: 4MB
	MaxRecvMsgSizeBytes int

	// StripeSecretKey is the Stripe secret API key (sk_live_... or sk_test_...).
	// Loaded from Vault secret/data/stripe key "secret_key", or STRIPE_SECRET_KEY.
	// If empty, Stripe-backed features (customer creation, webhook) are disabled.
	StripeSecretKey string

	// StripeWebhookSecret is the Stripe webhook endpoint secret (whsec_...).
	// Loaded from Vault secret/data/stripe key "webhook_secret", or STRIPE_WEBHOOK_SECRET.
	StripeWebhookSecret string
}

// Route maps an incoming gRPC service name prefix to an upstream address.
type Route struct {
	// ServicePrefix is the dotted service name prefix, e.g. "payments.v1".
	ServicePrefix string
	// UpstreamAddr is the host:port of the upstream gRPC server, e.g. "localhost:50051".
	UpstreamAddr string
}

// Load reads configuration from environment variables, consulting Vault first
// for sensitive values. Vault failure is non-fatal; env vars and defaults apply.
func Load() Config {
	vc := secrets.NewVaultClientImpl()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	dbURL := secrets.LoadOrEnv(ctx, vc, "secret/data/gateway", "database_url",
		"GATEWAY_DATABASE_URL", "")
	adminToken := secrets.LoadOrEnv(ctx, vc, "secret/data/gateway", "admin_token",
		"GATEWAY_ADMIN_TOKEN", "")
	stripeKey := secrets.LoadOrEnv(ctx, vc, "secret/data/stripe", "secret_key",
		"STRIPE_SECRET_KEY", "")
	stripeWHSecret := secrets.LoadOrEnv(ctx, vc, "secret/data/stripe", "webhook_secret",
		"STRIPE_WEBHOOK_SECRET", "")

	return Config{
		GRPCAddr:            envString("GATEWAY_GRPC_ADDR", ":50050"),
		AdminAddr:           envString("GATEWAY_ADMIN_ADDR", ":8080"),
		MetricsAddr:         envString("GATEWAY_METRICS_ADDR", ":9090"),
		DatabaseURL:         dbURL,
		AdminToken:          adminToken,
		Routes:              loadRoutes(),
		LogLevel:            envString("BLAZIL_LOG_LEVEL", "info"),
		ShutdownTimeout:     envDuration("GATEWAY_SHUTDOWN_TIMEOUT", 30*time.Second),
		MaxRecvMsgSizeBytes: envInt("GATEWAY_MAX_RECV_MSG_BYTES", 4*1024*1024),
		StripeSecretKey:     stripeKey,
		StripeWebhookSecret: stripeWHSecret,
	}
}

// loadRoutes builds the proxy route table from environment variables.
// Explicit GATEWAY_ROUTES takes precedence; otherwise per-service address vars are used.
//
// GATEWAY_ROUTES format: "payments.v1=localhost:50051,banking.v1=localhost:50052"
func loadRoutes() []Route {
	if raw := os.Getenv("GATEWAY_ROUTES"); raw != "" {
		return parseRoutes(raw)
	}
	// Conventional per-service defaults.
	defaults := []struct{ prefix, env, fallback string }{
		{"payments.v1", "PAYMENTS_GRPC_ADDR", "localhost:50051"},
		{"banking.v1", "BANKING_GRPC_ADDR", "localhost:50052"},
		{"trading.v1", "TRADING_GRPC_ADDR", "localhost:50053"},
		{"crypto.v1", "CRYPTO_GRPC_ADDR", "localhost:50054"},
	}
	routes := make([]Route, 0, len(defaults))
	for _, d := range defaults {
		routes = append(routes, Route{
			ServicePrefix: d.prefix,
			UpstreamAddr:  envString(d.env, d.fallback),
		})
	}
	return routes
}

// parseRoutes parses "prefix1=addr1,prefix2=addr2" into a Route slice.
func parseRoutes(raw string) []Route {
	var routes []Route
	for _, pair := range strings.Split(raw, ",") {
		parts := strings.SplitN(strings.TrimSpace(pair), "=", 2)
		if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
			continue
		}
		routes = append(routes, Route{ServicePrefix: parts[0], UpstreamAddr: parts[1]})
	}
	return routes
}

func envString(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func envInt(key string, fallback int) int {
	if v := os.Getenv(key); v != "" {
		if i, err := strconv.Atoi(v); err == nil {
			return i
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

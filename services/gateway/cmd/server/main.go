// Package main is the entry point for the Blazil API Gateway.
//
// The gateway runs three servers on separate addresses:
//   - :50050  gRPC reverse proxy (customer-facing)
//   - :8080   Admin REST control plane (internal only — must be firewalled)
//   - :9090   Prometheus metrics scrape endpoint
//
// Startup sequence:
//  1. Load configuration (Vault → env vars → defaults)
//  2. Connect to Postgres (pgxpool)
//  3. Initialize tenant store, metering recorder, usage writer, flusher
//  4. Build gRPC proxy server with interceptor chain
//  5. Start admin HTTP server
//  6. Start metrics HTTP server
//  7. Wait for SIGTERM or SIGINT
//  8. Graceful shutdown within cfg.ShutdownTimeout
package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/blazil/metering"
	"github.com/blazil/observability"
	"github.com/blazil/services/gateway/internal/admin"
	"github.com/blazil/services/gateway/internal/config"
	"github.com/blazil/services/gateway/internal/middleware"
	"github.com/blazil/services/gateway/internal/proxy"
	"github.com/blazil/services/gateway/internal/ratelimit"
	"github.com/blazil/services/gateway/internal/tenant"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	"go.uber.org/zap"
	"google.golang.org/grpc"
)

func main() {
	// ── Logger ──────────────────────────────────────────────────────────────
	env := os.Getenv("BLAZIL_ENV")
	if env == "" {
		env = "production"
	}
	logger := observability.NewLogger("gateway", env)
	defer logger.Sync() //nolint:errcheck

	if err := run(logger); err != nil {
		logger.Fatal("gateway startup failed", zap.Error(err))
	}
}

func run(logger *zap.Logger) error {
	// ── Configuration ────────────────────────────────────────────────────────
	cfg := config.Load()
	logger.Info("gateway starting",
		zap.String("grpc_addr", cfg.GRPCAddr),
		zap.String("admin_addr", cfg.AdminAddr),
		zap.String("metrics_addr", cfg.MetricsAddr),
	)

	// ── Postgres ─────────────────────────────────────────────────────────────
	if cfg.DatabaseURL == "" {
		return errors.New("GATEWAY_DATABASE_URL is required")
	}
	dbCtx, dbCancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer dbCancel()
	db, err := pgxpool.New(dbCtx, cfg.DatabaseURL)
	if err != nil {
		return fmt.Errorf("connect to postgres: %w", err)
	}
	defer db.Close()
	if err := db.Ping(dbCtx); err != nil {
		return fmt.Errorf("postgres ping: %w", err)
	}
	logger.Info("postgres connected")

	// ── Tenant store ─────────────────────────────────────────────────────────
	store := tenant.NewPGStore(db)

	// ── Metering ─────────────────────────────────────────────────────────────
	recorder := metering.NewRecorder()
	writer := &pgUsageWriter{db: db}
	flusher := metering.NewFlusher(recorder, writer, logger)

	// ── Root context (cancelled on OS signal) ───────────────────────────────
	rootCtx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()

	// Start metering flusher.
	go flusher.Run(rootCtx)

	// ── gRPC proxy server ────────────────────────────────────────────────────
	routes := make([]proxy.Route, len(cfg.Routes))
	for i, r := range cfg.Routes {
		routes[i] = proxy.Route{ServicePrefix: r.ServicePrefix, UpstreamAddr: r.UpstreamAddr}
	}
	director := proxy.NewDirector(routes)
	defer director.Close()

	limiter := ratelimit.NewLimiter()

	grpcServer := grpc.NewServer(
		grpc.MaxRecvMsgSize(cfg.MaxRecvMsgSizeBytes),
		grpc.UnknownServiceHandler(proxy.Handler(director)),
		grpc.ChainStreamInterceptor(
			middleware.AuthStreamInterceptor(store),
			middleware.RateLimitStreamInterceptor(limiter),
			middleware.MeteringStreamInterceptor(recorder),
		),
	)

	grpcLis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		return fmt.Errorf("listen grpc %s: %w", cfg.GRPCAddr, err)
	}

	// ── Admin HTTP server ────────────────────────────────────────────────────
	adminHandler := admin.New(store, writer, recorder, cfg.AdminToken, logger)
	adminServer := &http.Server{
		Addr:         cfg.AdminAddr,
		Handler:      adminHandler.Routes(),
		ReadTimeout:  15 * time.Second,
		WriteTimeout: 15 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	// ── Metrics HTTP server ──────────────────────────────────────────────────
	metricsMux := http.NewServeMux()
	metricsMux.Handle("/metrics", promhttp.Handler())
	metricsServer := &http.Server{
		Addr:        cfg.MetricsAddr,
		Handler:     metricsMux,
		ReadTimeout: 5 * time.Second,
	}

	// ── Start all servers ────────────────────────────────────────────────────
	errCh := make(chan error, 3)

	go func() {
		logger.Info("gRPC proxy listening", zap.String("addr", cfg.GRPCAddr))
		if err := grpcServer.Serve(grpcLis); err != nil {
			errCh <- fmt.Errorf("grpc server: %w", err)
		}
	}()

	go func() {
		logger.Info("admin REST listening", zap.String("addr", cfg.AdminAddr))
		if err := adminServer.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- fmt.Errorf("admin server: %w", err)
		}
	}()

	go func() {
		logger.Info("metrics listening", zap.String("addr", cfg.MetricsAddr))
		if err := metricsServer.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- fmt.Errorf("metrics server: %w", err)
		}
	}()

	// ── Wait for shutdown signal or fatal error ──────────────────────────────
	select {
	case <-rootCtx.Done():
		logger.Info("shutdown signal received")
	case err := <-errCh:
		return err
	}

	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), cfg.ShutdownTimeout)
	defer shutdownCancel()

	// Drain gRPC gracefully (stops accepting new RPCs, waits for in-flight to complete).
	grpcServer.GracefulStop()
	logger.Info("gRPC server stopped")

	if err := adminServer.Shutdown(shutdownCtx); err != nil {
		logger.Warn("admin server shutdown error", zap.Error(err))
	}
	if err := metricsServer.Shutdown(shutdownCtx); err != nil {
		logger.Warn("metrics server shutdown error", zap.Error(err))
	}

	logger.Info("gateway shutdown complete")
	return nil
}

// pgUsageWriter implements metering.UsageWriter backed by Postgres.
// Lives here (in main) so the metering lib stays backend-agnostic.
type pgUsageWriter struct {
	db *pgxpool.Pool
}

// UpsertUsage batch-upserts usage rows into the usage table using
// INSERT … ON CONFLICT DO UPDATE to guarantee idempotency (at-least-once delivery).
func (w *pgUsageWriter) UpsertUsage(ctx context.Context, rows []metering.UsageRow) error {
	if len(rows) == 0 {
		return nil
	}
	batch := &pgx.Batch{}
	for _, r := range rows {
		batch.Queue(`
			INSERT INTO usage (tenant_id, window_start, window_end, tx_count)
			VALUES ($1, $2, $3, $4)
			ON CONFLICT (tenant_id, window_start)
			DO UPDATE SET
				tx_count   = usage.tx_count + EXCLUDED.tx_count,
				window_end = EXCLUDED.window_end`,
			r.TenantID, r.WindowStart, r.WindowEnd, r.TxCount,
		)
	}
	br := w.db.SendBatch(ctx, batch)
	defer br.Close()
	for range rows {
		if _, err := br.Exec(); err != nil {
			return fmt.Errorf("upsert usage: %w", err)
		}
	}
	return nil
}

// MonthlyTotal returns the total transaction count for a tenant in the given month.
func (w *pgUsageWriter) MonthlyTotal(ctx context.Context, tenantID metering.TenantID, t time.Time) (int64, error) {
	start := time.Date(t.Year(), t.Month(), 1, 0, 0, 0, 0, time.UTC)
	end := start.AddDate(0, 1, 0)
	var total int64
	err := w.db.QueryRow(ctx, `
		SELECT COALESCE(SUM(tx_count), 0)
		FROM usage
		WHERE tenant_id = $1
		  AND window_start >= $2
		  AND window_start < $3`,
		string(tenantID), start, end,
	).Scan(&total)
	if err != nil {
		return 0, fmt.Errorf("monthly total: %w", err)
	}
	return total, nil
}

// WindowedUsage returns all usage rows for a tenant in the given year/month,
// ordered by window_start ascending.
func (w *pgUsageWriter) WindowedUsage(ctx context.Context, tenantID metering.TenantID, year, month int) ([]metering.UsageRow, error) {
	start := time.Date(year, time.Month(month), 1, 0, 0, 0, 0, time.UTC)
	end := start.AddDate(0, 1, 0)
	rows, err := w.db.Query(ctx, `
		SELECT tenant_id, window_start, window_end, tx_count
		FROM usage
		WHERE tenant_id = $1
		  AND window_start >= $2
		  AND window_start < $3
		ORDER BY window_start ASC`,
		string(tenantID), start, end,
	)
	if err != nil {
		return nil, fmt.Errorf("windowed usage: %w", err)
	}
	defer rows.Close()
	var result []metering.UsageRow
	for rows.Next() {
		var r metering.UsageRow
		var tid string
		if err := rows.Scan(&tid, &r.WindowStart, &r.WindowEnd, &r.TxCount); err != nil {
			return nil, fmt.Errorf("scan usage row: %w", err)
		}
		r.TenantID = metering.TenantID(tid)
		result = append(result, r)
	}
	return result, rows.Err()
}

// ensure pgUsageWriter satisfies the metering.UsageWriter interface at compile time.
var _ metering.UsageWriter = (*pgUsageWriter)(nil)

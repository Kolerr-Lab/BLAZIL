// Command server is the entrypoint for the Blazil payments gRPC service.
package main

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/blazil/observability"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	paymentsv1 "github.com/blazil/services/payments/api/proto/payments/v1"
	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/config"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/lifecycle"
	"github.com/blazil/services/payments/internal/routing"
)

func main() {
	cfg := config.Load()

	logger, err := buildLogger(cfg.LogLevel)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to build logger: %v\n", err)
		os.Exit(1)
	}
	defer logger.Sync() //nolint:errcheck

	logger.Info("starting blazil payments service",
		zap.String("grpc_addr", cfg.GRPCAddr),
		zap.String("engine_addr", cfg.EngineAddr),
	)

	// ── Observability ─────────────────────────────────────────────────────────
	if err := observability.RegisterAll(prometheus.DefaultRegisterer); err != nil {
		logger.Warn("metrics registration error", zap.Error(err))
	}
	otelShutdown, err := observability.InitTracer("payments", os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT"))
	if err != nil {
		logger.Warn("tracer init failed", zap.Error(err))
	} else {
		defer otelShutdown()
	}
	go func() {
		mux := http.NewServeMux()
		mux.Handle("/metrics", observability.MetricsHandler())
		if err := http.ListenAndServe(cfg.MetricsAddr, mux); err != nil {
			logger.Error("metrics server error", zap.Error(err))
		}
	}()

	// ── Wiring ────────────────────────────────────────────────────────────────

	authCfg := authorization.DefaultAuthorizerConfig()
	authCfg.MaxAmountMinorUnits = cfg.MaxAmountMinorUnits
	auth := authorization.NewCompositeAuthorizer(authCfg)

	routerCfg := routing.DefaultRouterConfig()
	router := routing.NewRuleBasedRouter(routerCfg)

	idempotencyStore := lifecycle.NewInMemoryIdempotencyStore(cfg.IdempotencyTTL)
	cleanupDone := make(chan struct{})
	idempotencyStore.StartCleanup(time.Hour, cleanupDone)

	engineClient := engine.NewTcpEngineClient(cfg.EngineAddr, cfg.EngineTimeout)

	paymentStore := lifecycle.NewInMemoryPaymentStore()
	processor := lifecycle.NewPaymentProcessor(paymentStore, auth, router, idempotencyStore, engineClient)

	// ── gRPC server ───────────────────────────────────────────────────────────

	lis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		logger.Fatal("failed to listen", zap.String("addr", cfg.GRPCAddr), zap.Error(err))
	}

	grpcServer := grpc.NewServer(
		grpc.UnaryInterceptor(observability.UnaryServerInterceptor("payments")),
	)
	paymentsv1.RegisterPaymentsServiceServer(grpcServer, &paymentsServer{
		processor: processor,
		logger:    logger,
	})

	// ── Graceful shutdown ─────────────────────────────────────────────────────

	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		logger.Info("gRPC server listening", zap.String("addr", cfg.GRPCAddr))
		if err := grpcServer.Serve(lis); err != nil {
			logger.Error("gRPC server error", zap.Error(err))
		}
	}()

	<-quit
	logger.Info("shutting down")
	grpcServer.GracefulStop()
	close(cleanupDone)
	logger.Info("shutdown complete")
}

// ── gRPC service implementation ───────────────────────────────────────────────

// paymentsServer implements paymentsv1.PaymentsServiceServer.
type paymentsServer struct {
	paymentsv1.UnimplementedPaymentsServiceServer
	processor *lifecycle.PaymentProcessor
	logger    *zap.Logger
}

// ProcessPayment implements PaymentsServiceServer.
func (s *paymentsServer) ProcessPayment(
	ctx context.Context,
	req *paymentsv1.ProcessPaymentRequest,
) (*paymentsv1.ProcessPaymentResponse, error) {
	currency, err := domain.CurrencyByCode(req.CurrencyCode)
	if err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "unsupported currency: %s", req.CurrencyCode)
	}

	domainReq := domain.ProcessPaymentRequest{
		IdempotencyKey:  req.IdempotencyKey,
		DebitAccountID:  domain.AccountID(req.DebitAccountId),
		CreditAccountID: domain.AccountID(req.CreditAccountId),
		Amount:          domain.NewMoney(req.AmountMinorUnits, currency),
		LedgerID:        domain.LedgerID(req.LedgerId),
		Metadata:        req.Metadata,
	}

	payment, err := s.processor.Process(ctx, domainReq)
	if err != nil {
		s.logger.Error("payment processing failed", zap.Error(err), zap.String("idempotency_key", req.IdempotencyKey))
		return nil, status.Errorf(codes.Internal, "internal error: %v", err)
	}

	return &paymentsv1.ProcessPaymentResponse{
		PaymentId:     string(payment.ID),
		Status:        payment.Status.String(),
		Rails:         payment.Rails.String(),
		FailureReason: payment.FailureReason,
	}, nil
}

// GetPayment implements PaymentsServiceServer.
func (s *paymentsServer) GetPayment(
	ctx context.Context,
	req *paymentsv1.GetPaymentRequest,
) (*paymentsv1.GetPaymentResponse, error) {
	payment, err := s.processor.GetPayment(domain.PaymentID(req.PaymentId))
	if err != nil {
		return nil, status.Errorf(codes.NotFound, "payment not found: %s", req.PaymentId)
	}

	return &paymentsv1.GetPaymentResponse{
		PaymentId:        string(payment.ID),
		Status:           payment.Status.String(),
		Rails:            payment.Rails.String(),
		AmountMinorUnits: payment.Amount.MinorUnits,
		CurrencyCode:     payment.Amount.Currency.Code,
	}, nil
}

// ── helpers ───────────────────────────────────────────────────────────────────

func buildLogger(level string) (*zap.Logger, error) {
	switch level {
	case "debug":
		return zap.NewDevelopment()
	default:
		return zap.NewProduction()
	}
}

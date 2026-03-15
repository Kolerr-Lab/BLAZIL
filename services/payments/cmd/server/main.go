// Command server is the entrypoint for the Blazil payments gRPC service.
package main

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	blazilauth "github.com/blazil/auth"
	"github.com/blazil/observability"
	"github.com/blazil/sharding"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
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

	// ── Sharding coordinator (multi-node mode) ────────────────────────────────
	if cfg.ShardingEnabled && len(cfg.NodeAddresses) > 0 {
		ring := &sharding.NodeRing{}
		for i, raw := range cfg.NodeAddresses {
			// BLAZIL_NODES format: "nodeID:host:port" — split on first colon.
			parts := strings.SplitN(raw, ":", 2)
			nodeID := fmt.Sprintf("node-%d", i)
			addr := raw
			if len(parts) == 2 {
				nodeID = parts[0]
				addr = parts[1]
			}
			_ = ring.Add(sharding.NodeInfo{
				ID:      nodeID,
				Address: addr,
				ShardID: i,
				Status:  sharding.NodeStatusHealthy,
			})
		}
		shardRouter := sharding.NewShardRouter(ring, len(cfg.NodeAddresses))
		balancer := sharding.NewShardAwareLoadBalancer(shardRouter, ring)
		processor.SetShardRouter(shardRouter)
		factory := func(nodeAddress string) sharding.NodeTransferClient {
			//nolint:staticcheck
			conn, err := grpc.Dial(nodeAddress, grpc.WithTransportCredentials(insecure.NewCredentials()))
			if err != nil {
				logger.Error("sharding: failed to dial node", zap.String("addr", nodeAddress), zap.Error(err))
				return engine.NewTcpTransferClient(conn)
			}
			return engine.NewTcpTransferClient(conn)
		}
		coordinator := sharding.NewCrossShardCoordinator(shardRouter, balancer, factory)
		processor.SetCrossShardCoordinator(coordinator)
		logger.Info("sharding enabled",
			zap.Int("shards", len(cfg.NodeAddresses)),
			zap.Strings("nodes", cfg.NodeAddresses),
		)
	}

	// ── gRPC server ───────────────────────────────────────────────────────────

	lis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		logger.Fatal("failed to listen", zap.String("addr", cfg.GRPCAddr), zap.Error(err))
	}

	grpcServer := grpc.NewServer(
		grpc.ChainUnaryInterceptor(
			observability.UnaryServerInterceptor("payments"),
			blazilauth.AuthInterceptor(blazilauth.NewJWTValidator()),
		),
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

// ProcessPaymentStream implements bidirectional streaming for high-throughput
// payment processing. Eliminates per-request RTT overhead, enabling 50,000+ TPS.
// Order is preserved: response N corresponds to request N.
func (s *paymentsServer) ProcessPaymentStream(
	stream paymentsv1.PaymentsService_ProcessPaymentStreamServer,
) error {
	for {
		req, err := stream.Recv()
		if err == io.EOF {
			// Client closed the stream, we're done.
			return nil
		}
		if err != nil {
			s.logger.Error("stream receive error", zap.Error(err))
			return status.Errorf(codes.Internal, "stream error: %v", err)
		}

		// Process payment (same logic as unary handler).
		currency, err := domain.CurrencyByCode(req.CurrencyCode)
		if err != nil {
			// Send error response on stream instead of returning.
			resp := &paymentsv1.ProcessPaymentResponse{
				PaymentId:     "",
				Status:        "failed",
				FailureReason: fmt.Sprintf("unsupported currency: %s", req.CurrencyCode),
			}
			if sendErr := stream.Send(resp); sendErr != nil {
				return status.Errorf(codes.Internal, "failed to send error response: %v", sendErr)
			}
			continue
		}

		domainReq := domain.ProcessPaymentRequest{
			IdempotencyKey:  req.IdempotencyKey,
			DebitAccountID:  domain.AccountID(req.DebitAccountId),
			CreditAccountID: domain.AccountID(req.CreditAccountId),
			Amount:          domain.NewMoney(req.AmountMinorUnits, currency),
			LedgerID:        domain.LedgerID(req.LedgerId),
			Metadata:        req.Metadata,
		}

		payment, err := s.processor.Process(stream.Context(), domainReq)
		if err != nil {
			s.logger.Error("payment processing failed (stream)", zap.Error(err), zap.String("idempotency_key", req.IdempotencyKey))
			resp := &paymentsv1.ProcessPaymentResponse{
				PaymentId:     "",
				Status:        "failed",
				FailureReason: fmt.Sprintf("internal error: %v", err),
			}
			if sendErr := stream.Send(resp); sendErr != nil {
				return status.Errorf(codes.Internal, "failed to send error response: %v", sendErr)
			}
			continue
		}

		// Send success response on stream.
		resp := &paymentsv1.ProcessPaymentResponse{
			PaymentId:     string(payment.ID),
			Status:        payment.Status.String(),
			Rails:         payment.Rails.String(),
			FailureReason: payment.FailureReason,
		}
		if err := stream.Send(resp); err != nil {
			return status.Errorf(codes.Internal, "failed to send response: %v", err)
		}
	}
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

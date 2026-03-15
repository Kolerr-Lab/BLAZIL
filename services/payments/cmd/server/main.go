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
	"sync/atomic"
	"syscall"
	"time"

	"golang.org/x/sync/semaphore"

	blazilauth "github.com/blazil/auth"
	"github.com/blazil/observability"
	"github.com/blazil/sharding"
	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promauto"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/keepalive"
	"google.golang.org/grpc/status"

	paymentsv1 "github.com/blazil/services/payments/api/proto/payments/v1"
	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/config"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/lifecycle"
	"github.com/blazil/services/payments/internal/routing"
)

// ── Metrics ───────────────────────────────────────────────────────────────────

var (
	// streamResponsesDropped tracks responses dropped due to backpressure.
	// When client drains slower than server produces, oldest responses are dropped (LIFO).
	streamResponsesDropped = promauto.NewCounter(prometheus.CounterOpts{
		Name: "blazil_payments_stream_responses_dropped_total",
		Help: "Total number of streaming responses dropped due to buffer overflow (backpressure)",
	})

	// concurrencyLimitReached tracks requests rejected due to semaphore limit.
	concurrencyLimitReached = promauto.NewCounter(prometheus.CounterOpts{
		Name: "blazil_payments_concurrency_limit_reached_total",
		Help: "Total number of requests rejected due to concurrent processing limit",
	})
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

	// FIX 3: gRPC server with aggressive keepalive parameters
	grpcServer := grpc.NewServer(
		grpc.ChainUnaryInterceptor(
			observability.UnaryServerInterceptor("payments"),
			blazilauth.AuthInterceptor(blazilauth.NewJWTValidator()),
		),
		grpc.KeepaliveParams(keepalive.ServerParameters{
			MaxConnectionIdle:     15 * time.Second,
			MaxConnectionAge:      30 * time.Second,
			MaxConnectionAgeGrace: 5 * time.Second,
			Time:                  5 * time.Second,
			Timeout:               1 * time.Second,
		}),
	)

	// BACKPRESSURE PROTECTION: Limit max concurrent requests to prevent OOM at high load.
	// 1 goroutine × 256 window = 256 in-flight + 44 buffer = 300 max concurrent.
	// System crashes at 3+ goroutines (768+ concurrent) — reject excess immediately.
	const maxConcurrent = 300
	concurrencySem := semaphore.NewWeighted(maxConcurrent)
	logger.Info("backpressure protection enabled", zap.Int64("max_concurrent", maxConcurrent))

	paymentsv1.RegisterPaymentsServiceServer(grpcServer, &paymentsServer{
		processor:      processor,
		logger:         logger,
		concurrencySem: concurrencySem,
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
	processor      *lifecycle.PaymentProcessor
	logger         *zap.Logger
	concurrencySem *semaphore.Weighted // Limits max concurrent requests (prevents OOM)
}

// ── Ring Buffer (LIFO drop strategy) ──────────────────────────────────────────

// responseRingBuffer implements a fixed-capacity ring buffer with drop-oldest semantics.
// When full, new entries overwrite the oldest (LIFO policy). This prevents OOM under
// backpressure when clients drain slower than the server produces.
type responseRingBuffer struct {
	buf     []*paymentsv1.ProcessPaymentResponse
	head    int       // Next write position
	tail    int       // Next read position
	size    int       // Current number of entries
	cap     int       // Maximum capacity
	dropped int64     // Atomic counter for dropped messages
}

func newResponseRingBuffer(capacity int) *responseRingBuffer {
	return &responseRingBuffer{
		buf: make([]*paymentsv1.ProcessPaymentResponse, capacity),
		cap: capacity,
	}
}

// push adds a response to the buffer. If full, drops the oldest entry (LIFO).
// Never blocks — always accepts new entries.
func (rb *responseRingBuffer) push(resp *paymentsv1.ProcessPaymentResponse) {
	if rb.size == rb.cap {
		// Buffer full: drop oldest (advance tail)
		rb.tail = (rb.tail + 1) % rb.cap
		rb.size--
		atomic.AddInt64(&rb.dropped, 1)
		streamResponsesDropped.Inc()
	}

	rb.buf[rb.head] = resp
	rb.head = (rb.head + 1) % rb.cap
	rb.size++
}

// pop removes and returns the oldest entry. Returns nil if empty.
func (rb *responseRingBuffer) pop() *paymentsv1.ProcessPaymentResponse {
	if rb.size == 0 {
		return nil
	}

	resp := rb.buf[rb.tail]
	rb.tail = (rb.tail + 1) % rb.cap
	rb.size--
	return resp
}

// len returns the current number of buffered responses.
func (rb *responseRingBuffer) len() int {
	return rb.size
}

// droppedCount returns the total number of dropped responses.
func (rb *responseRingBuffer) droppedCount() int64 {
	return atomic.LoadInt64(&rb.dropped)
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

// ProcessPaymentStream implements bidirectional streaming with backpressure protection.
//
// Uses a fixed-capacity ring buffer (1000 entries) with drop-oldest policy. When
// client drains slower than server produces, oldest responses are dropped to prevent
// OOM. Dropped count is tracked in Prometheus metrics.
//
// Architecture:
//   - Main goroutine: Recv() → Process() → Push to buffer (never blocks)
//   - Sender goroutine: Pop from buffer → Send() (blocks on slow client)
//   - If buffer full: Drop oldest, increment counter, continue
//
// This is the LMAX Disruptor strategy: producer never blocks, slow consumers
// lose data rather than crashing the system.
func (s *paymentsServer) ProcessPaymentStream(
	stream paymentsv1.PaymentsService_ProcessPaymentStreamServer,
) error {
	const bufferCapacity = 1000

	respBuf := newResponseRingBuffer(bufferCapacity)
	done := make(chan struct{})
	senderErr := make(chan error, 1)

	// Sender goroutine: drains buffer and sends to stream.
	// Blocks on stream.Send() if client is slow — this is intentional.
	// Buffer absorbs bursts; drops prevent OOM.
	go func() {
		defer close(done)
		for {
			select {
			case <-stream.Context().Done():
				return
			default:
			}

			resp := respBuf.pop()
			if resp == nil {
				// Buffer empty, wait a bit before polling again
				time.Sleep(100 * time.Microsecond)
				continue
			}

			if err := stream.Send(resp); err != nil {
				senderErr <- err
				return
			}
		}
	}()

	// Main loop: receive requests, process, push to buffer (never blocks).
	for {
		req, err := stream.Recv()
		if err == io.EOF {
			// Client closed stream — drain buffer and exit.
			break
		}
		if err != nil {
			s.logger.Error("stream receive error", zap.Error(err))
			return status.Errorf(codes.Internal, "stream error: %v", err)
		}

		// Check if sender goroutine failed.
		select {
		case err := <-senderErr:
			return status.Errorf(codes.Internal, "sender error: %v", err)
		default:
		}

		// BACKPRESSURE: Try acquire semaphore (non-blocking).
		// If at capacity, reject immediately with ResourceExhausted.
		if !s.concurrencySem.TryAcquire(1) {
			concurrencyLimitReached.Inc()
			resp := &paymentsv1.ProcessPaymentResponse{
				PaymentId:     "",
				Status:        "failed",
				FailureReason: "server overloaded: max concurrent requests exceeded",
			}
			respBuf.push(resp)
			continue
		}

		// Process payment (same logic as unary handler).
		currency, err := domain.CurrencyByCode(req.CurrencyCode)
		if err != nil {
			resp := &paymentsv1.ProcessPaymentResponse{
				PaymentId:     "",
				Status:        "failed",
				FailureReason: fmt.Sprintf("unsupported currency: %s", req.CurrencyCode),
			}
			respBuf.push(resp)
			s.concurrencySem.Release(1)
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
			respBuf.push(resp)
			s.concurrencySem.Release(1)
			continue
		}

		// Push success response to buffer (never blocks).
		resp := &paymentsv1.ProcessPaymentResponse{
			PaymentId:     string(payment.ID),
			Status:        payment.Status.String(),
			Rails:         payment.Rails.String(),
			FailureReason: payment.FailureReason,
		}
		respBuf.push(resp)
		s.concurrencySem.Release(1)
	}

	// Client closed stream — wait for sender to drain remaining responses.
	// Give it 5 seconds max, then force shutdown.
	drainTimeout := time.After(5 * time.Second)
	for respBuf.len() > 0 {
		select {
		case <-drainTimeout:
			s.logger.Warn("stream drain timeout", zap.Int("remaining", respBuf.len()))
			goto shutdown
		case <-done:
			// Sender exited (probably error)
			goto shutdown
		default:
			time.Sleep(10 * time.Millisecond)
		}
	}

shutdown:
	// Log backpressure statistics.
	dropped := respBuf.droppedCount()
	if dropped > 0 {
		s.logger.Warn("stream completed with dropped responses",
			zap.Int64("dropped", dropped),
			zap.Int("buffer_capacity", bufferCapacity),
		)
	}

	<-done // Wait for sender goroutine to exit
	return nil
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

// Command server is the entrypoint for the Blazil trading gRPC service.
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

	"github.com/blazil/observability"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	tradingv1 "github.com/blazil/trading/api/proto/trading/v1"
	"github.com/blazil/trading/internal/config"
	"github.com/blazil/trading/internal/domain"
	"github.com/blazil/trading/internal/matching"
	"github.com/blazil/trading/internal/orders"
	"github.com/blazil/trading/internal/positions"
	"github.com/blazil/trading/internal/settlement"
)

func main() {
	cfg := config.Load()

	logger := observability.NewLogger("trading", cfg.LogLevel)
	defer logger.Sync() //nolint:errcheck

	// ── Observability ─────────────────────────────────────────────────────────
	if err := observability.RegisterAll(prometheus.DefaultRegisterer); err != nil {
		logger.Warn("metrics registration error", zap.Error(err))
	}
	otelShutdown, err := observability.InitTracer("trading", os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT"))
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

	orderSvc := orders.NewInMemoryOrderService(matching.NewFIFOEngine())
	posSvc := positions.NewInMemoryPositionService()
	settler := settlement.NewEngineSettler(orderSvc, posSvc)

	lis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to listen on %s: %v\n", cfg.GRPCAddr, err)
		os.Exit(1)
	}

	grpcServer := grpc.NewServer(
		grpc.UnaryInterceptor(observability.UnaryServerInterceptor("trading")),
	)
	tradingv1.RegisterTradingServiceServer(grpcServer, &tradingServer{
		orders:   orderSvc,
		positions: posSvc,
		settler:  settler,
	})

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
}

// tradingServer implements tradingv1.TradingServiceServer.
type tradingServer struct {
	tradingv1.UnimplementedTradingServiceServer
	orders    *orders.InMemoryOrderService
	positions *positions.InMemoryPositionService
	settler   *settlement.EngineSettler
}

// PlaceOrder implements TradingServiceServer.
func (s *tradingServer) PlaceOrder(ctx context.Context, req *tradingv1.PlaceOrderRequest) (*tradingv1.PlaceOrderResponse, error) {
	side, err := parseSide(req.Side)
	if err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid side: %v", err)
	}

	order, trades, err := s.orders.PlaceOrder(ctx, orders.PlaceOrderRequest{
		ID:                   domain.OrderID(req.OrderId),
		InstrumentID:         domain.InstrumentID(req.InstrumentId),
		OwnerID:              req.OwnerId,
		Side:                 side,
		LimitPriceMinorUnits: req.LimitPriceMinorUnits,
		QuantityUnits:        req.QuantityUnits,
	})
	if err != nil {
		return nil, domainToGRPCStatus(err)
	}

	if err := s.settler.Settle(ctx, trades); err != nil {
		return nil, status.Errorf(codes.Internal, "settle: %v", err)
	}

	protoTrades := make([]*tradingv1.TradeProto, 0, len(trades))
	for _, t := range trades {
		protoTrades = append(protoTrades, tradeToProto(t))
	}

	return &tradingv1.PlaceOrderResponse{
		Order:  orderToProto(order),
		Trades: protoTrades,
	}, nil
}

// CancelOrder implements TradingServiceServer.
func (s *tradingServer) CancelOrder(ctx context.Context, req *tradingv1.CancelOrderRequest) (*tradingv1.CancelOrderResponse, error) {
	if err := s.orders.CancelOrder(ctx, domain.OrderID(req.OrderId)); err != nil {
		return nil, domainToGRPCStatus(err)
	}
	order, _ := s.orders.GetOrder(ctx, domain.OrderID(req.OrderId))
	return &tradingv1.CancelOrderResponse{Order: orderToProto(order)}, nil
}

// GetOrder implements TradingServiceServer.
func (s *tradingServer) GetOrder(ctx context.Context, req *tradingv1.GetOrderRequest) (*tradingv1.GetOrderResponse, error) {
	order, err := s.orders.GetOrder(ctx, domain.OrderID(req.OrderId))
	if err != nil {
		return nil, domainToGRPCStatus(err)
	}
	return &tradingv1.GetOrderResponse{Order: orderToProto(order)}, nil
}

// GetPosition implements TradingServiceServer.
func (s *tradingServer) GetPosition(ctx context.Context, req *tradingv1.GetPositionRequest) (*tradingv1.GetPositionResponse, error) {
	pos, err := s.positions.GetPosition(ctx, req.OwnerId, domain.InstrumentID(req.InstrumentId))
	if err != nil {
		return nil, domainToGRPCStatus(err)
	}
	return &tradingv1.GetPositionResponse{Position: positionToProto(pos)}, nil
}

// ListPositions implements TradingServiceServer.
func (s *tradingServer) ListPositions(ctx context.Context, req *tradingv1.ListPositionsRequest) (*tradingv1.ListPositionsResponse, error) {
	ps, err := s.positions.ListByOwner(ctx, req.OwnerId)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "list positions: %v", err)
	}
	protos := make([]*tradingv1.PositionProto, 0, len(ps))
	for _, p := range ps {
		protos = append(protos, positionToProto(p))
	}
	return &tradingv1.ListPositionsResponse{Positions: protos}, nil
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

func parseSide(s string) (domain.Side, error) {
	switch s {
	case "buy":
		return domain.SideBuy, nil
	case "sell":
		return domain.SideSell, nil
	default:
		return 0, fmt.Errorf("unknown side %q", s)
	}
}

func domainToGRPCStatus(err error) error {
	switch {
	case errors.Is(err, domain.ErrOrderNotFound), errors.Is(err, domain.ErrPositionNotFound), errors.Is(err, domain.ErrInstrumentNotFound):
		return status.Errorf(codes.NotFound, "%v", err)
	case errors.Is(err, domain.ErrOrderAlreadyExists), errors.Is(err, domain.ErrInstrumentAlreadyExists):
		return status.Errorf(codes.AlreadyExists, "%v", err)
	case errors.Is(err, domain.ErrOrderNotOpen):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrInvalidQuantity), errors.Is(err, domain.ErrInvalidPrice), errors.Is(err, domain.ErrUnknownSide):
		return status.Errorf(codes.InvalidArgument, "%v", err)
	default:
		return status.Errorf(codes.Internal, "%v", err)
	}
}

func orderToProto(o *domain.Order) *tradingv1.OrderProto {
	if o == nil {
		return nil
	}
	return &tradingv1.OrderProto{
		OrderId:              string(o.ID),
		InstrumentId:         string(o.InstrumentID),
		OwnerId:              o.OwnerID,
		Side:                 o.Side.String(),
		LimitPriceMinorUnits: o.LimitPriceMinorUnits,
		QuantityUnits:        o.QuantityUnits,
		FilledUnits:          o.FilledUnits,
		Status:               o.Status.String(),
		PlacedAtUnixNano:     o.PlacedAt.UnixNano(),
	}
}

func tradeToProto(t domain.Trade) *tradingv1.TradeProto {
	return &tradingv1.TradeProto{
		TradeId:            string(t.ID),
		InstrumentId:       string(t.InstrumentID),
		MakerOrderId:       string(t.MakerOrderID),
		TakerOrderId:       string(t.TakerOrderID),
		PriceMinorUnits:    t.PriceMinorUnits,
		QuantityUnits:      t.QuantityUnits,
		ExecutedAtUnixNano: t.ExecutedAt.UnixNano(),
	}
}

func positionToProto(p *domain.Position) *tradingv1.PositionProto {
	if p == nil {
		return nil
	}
	return &tradingv1.PositionProto{
		PositionId:            string(p.ID),
		OwnerId:               p.OwnerID,
		InstrumentId:          string(p.InstrumentID),
		QuantityUnits:         p.QuantityUnits,
		AverageCostMinorUnits: p.AverageCostMinorUnits,
	}
}


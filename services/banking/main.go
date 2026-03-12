// Command server is the entrypoint for the Blazil banking gRPC service.
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

	"github.com/blazil/observability"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	bankingv1 "github.com/blazil/banking/api/proto/banking/v1"
	"github.com/blazil/banking/internal/accounts"
	"github.com/blazil/banking/internal/balances"
	"github.com/blazil/banking/internal/config"
	"github.com/blazil/banking/internal/domain"
	"github.com/blazil/banking/internal/history"
)

func main() {
	cfg := config.Load()

	logger, err := buildLogger(cfg.LogLevel)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to build logger: %v\n", err)
		os.Exit(1)
	}
	defer logger.Sync() //nolint:errcheck

	logger.Info("starting blazil banking service", zap.String("grpc_addr", cfg.GRPCAddr))

	// ── Observability ─────────────────────────────────────────────────────────
	if err := observability.RegisterAll(prometheus.DefaultRegisterer); err != nil {
		logger.Warn("metrics registration error", zap.Error(err))
	}
	otelShutdown, err := observability.InitTracer("banking", os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT"))
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

	accountSvc := accounts.NewInMemoryAccountService()
	balanceStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()
	balanceSvc := balances.NewAccountBalanceService(accountSvc, balanceStore, txStore)
	accountSvc.SetBalanceService(balanceSvc)

	// ── gRPC server ───────────────────────────────────────────────────────────

	lis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		logger.Fatal("failed to listen", zap.String("addr", cfg.GRPCAddr), zap.Error(err))
	}

	grpcServer := grpc.NewServer(
		grpc.UnaryInterceptor(observability.UnaryServerInterceptor("banking")),
	)
	bankingv1.RegisterBankingServiceServer(grpcServer, &bankingServer{
		accounts:     accountSvc,
		balances:     balanceSvc,
		balanceStore: balanceStore,
		history:      txStore,
		logger:       logger,
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
}

// bankingServer implements bankingv1.BankingServiceServer.
type bankingServer struct {
	bankingv1.UnimplementedBankingServiceServer
	accounts     *accounts.InMemoryAccountService
	balances     *balances.AccountBalanceService
	balanceStore *balances.InMemoryBalanceStore
	history      *history.InMemoryTransactionStore
	logger       *zap.Logger
}

// CreateAccount implements BankingServiceServer.
func (s *bankingServer) CreateAccount(ctx context.Context, req *bankingv1.CreateAccountRequest) (*bankingv1.CreateAccountResponse, error) {
	accType, err := parseAccountType(req.AccountType)
	if err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "invalid account_type: %v", err)
	}

	acc, err := s.accounts.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID:                       domain.AccountID(req.AccountId),
		OwnerID:                  req.OwnerId,
		Type:                     accType,
		CurrencyCode:             req.CurrencyCode,
		InitialBalanceMinorUnits: req.InitialBalanceMinorUnits,
	})
	if err != nil {
		s.logger.Warn("CreateAccount failed", zap.String("id", req.AccountId), zap.Error(err))
		return nil, domainToGRPCStatus(err)
	}

	// Seed the balance store so GetBalance works immediately after account creation.
	if err := s.balanceStore.Set(ctx, &domain.Balance{
		AccountID:    acc.ID,
		MinorUnits:   acc.BalanceMinorUnits,
		CurrencyCode: acc.CurrencyCode,
		UpdatedAt:    time.Now().UTC(),
	}); err != nil {
		s.logger.Warn("seed balance store failed", zap.String("id", req.AccountId), zap.Error(err))
	}

	return &bankingv1.CreateAccountResponse{
		AccountId:    string(acc.ID),
		Status:       acc.Status.String(),
		CurrencyCode: acc.CurrencyCode,
	}, nil
}

// GetAccount implements BankingServiceServer.
func (s *bankingServer) GetAccount(ctx context.Context, req *bankingv1.GetAccountRequest) (*bankingv1.GetAccountResponse, error) {
	acc, err := s.accounts.GetAccount(ctx, domain.AccountID(req.AccountId))
	if err != nil {
		return nil, domainToGRPCStatus(err)
	}
	return &bankingv1.GetAccountResponse{
		AccountId:         string(acc.ID),
		OwnerId:           acc.OwnerID,
		AccountType:       acc.Type.String(),
		CurrencyCode:      acc.CurrencyCode,
		BalanceMinorUnits: acc.BalanceMinorUnits,
		Status:            acc.Status.String(),
	}, nil
}

// GetBalance implements BankingServiceServer.
func (s *bankingServer) GetBalance(ctx context.Context, req *bankingv1.GetBalanceRequest) (*bankingv1.GetBalanceResponse, error) {
	summary, err := s.balances.GetBalanceSummary(ctx, domain.AccountID(req.AccountId))
	if err != nil {
		return nil, domainToGRPCStatus(err)
	}
	return &bankingv1.GetBalanceResponse{
		AccountId:         string(summary.AccountID),
		BalanceMinorUnits: summary.BalanceMinorUnits,
		CurrencyCode:      summary.CurrencyCode,
		Status:            summary.Status.String(),
	}, nil
}

// ListTransactions implements BankingServiceServer.
func (s *bankingServer) ListTransactions(ctx context.Context, req *bankingv1.ListTransactionsRequest) (*bankingv1.ListTransactionsResponse, error) {
	txs, err := s.history.ListByAccount(ctx, domain.AccountID(req.AccountId), history.ListOptions{
		Limit:  int(req.Limit),
		Offset: int(req.Offset),
	})
	if err != nil {
		return nil, status.Errorf(codes.Internal, "list transactions: %v", err)
	}

	protos := make([]*bankingv1.TransactionProto, 0, len(txs))
	for _, tx := range txs {
		protos = append(protos, &bankingv1.TransactionProto{
			TransactionId:          string(tx.ID),
			AccountId:              string(tx.AccountID),
			Type:                   tx.Type.String(),
			AmountMinorUnits:       tx.AmountMinorUnits,
			CurrencyCode:           tx.CurrencyCode,
			BalanceAfterMinorUnits: tx.BalanceAfterMinorUnits,
			Description:            tx.Description,
			TimestampUnixNano:      tx.Timestamp.UnixNano(),
		})
	}
	return &bankingv1.ListTransactionsResponse{Transactions: protos}, nil
}

func buildLogger(level string) (*zap.Logger, error) {
	if level == "development" || level == "dev" {
		return zap.NewDevelopment()
	}
	return zap.NewProduction()
}

func parseAccountType(s string) (domain.AccountType, error) {
	switch s {
	case "checking", "":
		return domain.AccountTypeChecking, nil
	case "savings":
		return domain.AccountTypeSavings, nil
	case "loan":
		return domain.AccountTypeLoan, nil
	default:
		return 0, fmt.Errorf("unknown account type: %q", s)
	}
}

func domainToGRPCStatus(err error) error {
	switch {
	case errors.Is(err, domain.ErrAccountNotFound), errors.Is(err, domain.ErrTransactionNotFound):
		return status.Errorf(codes.NotFound, "%v", err)
	case errors.Is(err, domain.ErrAccountAlreadyExists):
		return status.Errorf(codes.AlreadyExists, "%v", err)
	case errors.Is(err, domain.ErrInsufficientFunds):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrAccountClosed):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrAccountFrozen):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrAccountNotActive):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrAccountHasBalance):
		return status.Errorf(codes.FailedPrecondition, "%v", err)
	case errors.Is(err, domain.ErrNegativeAmount):
		return status.Errorf(codes.InvalidArgument, "%v", err)
	default:
		return status.Errorf(codes.Internal, "%v", err)
	}
}


// Command server is the entrypoint for the Blazil crypto gRPC service.
package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/signal"
	"syscall"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"

	cryptov1 "github.com/blazil/crypto/api/proto/crypto/v1"
	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/config"
	"github.com/blazil/crypto/internal/deposits"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/transfers"
	"github.com/blazil/crypto/internal/wallets"
	"github.com/blazil/crypto/internal/withdrawals"
)

func main() {
	cfg := config.Load()

	// Build chain registry with mock adapters (no real blockchain connections).
	registry := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		registry.Register(chains.NewMockChainAdapter(c))
	}

	walletStore := wallets.NewInMemoryWalletStore()
	walletSvc := wallets.NewInMemoryWalletService(walletStore, registry)

	depositStore := deposits.NewInMemoryDepositStore()
	depositDetector := deposits.NewInMemoryDepositDetector(depositStore)
	depositProcessor := deposits.NewDepositProcessor(depositStore, registry, nil)

	withdrawalStore := withdrawals.NewInMemoryWithdrawalStore()
	withdrawalSvc := withdrawals.NewInMemoryWithdrawalService(withdrawalStore, registry, nil)

	transferSvc := transfers.NewInMemoryInternalTransferService(walletSvc, nil)

	lis, err := net.Listen("tcp", cfg.GRPCAddr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "failed to listen on %s: %v\n", cfg.GRPCAddr, err)
		os.Exit(1)
	}

	grpcServer := grpc.NewServer()
	cryptov1.RegisterCryptoServiceServer(grpcServer, &cryptoServer{
		walletSvc:       walletSvc,
		depositStore:    depositStore,
		depositDetector: depositDetector,
		depositProc:     depositProcessor,
		withdrawalSvc:   withdrawalSvc,
		transferSvc:     transferSvc,
	})

	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-quit
		grpcServer.GracefulStop()
	}()

	fmt.Printf("crypto gRPC server listening on %s\n", cfg.GRPCAddr)
	if err := grpcServer.Serve(lis); err != nil {
		fmt.Fprintf(os.Stderr, "grpc serve: %v\n", err)
		os.Exit(1)
	}
}

type cryptoServer struct {
	cryptov1.UnimplementedCryptoServiceServer
	walletSvc       *wallets.InMemoryWalletService
	depositStore    *deposits.InMemoryDepositStore
	depositDetector *deposits.InMemoryDepositDetector
	depositProc     *deposits.DepositProcessor
	withdrawalSvc   *withdrawals.InMemoryWithdrawalService
	transferSvc     *transfers.InMemoryInternalTransferService
}

// CreateWallet implements CryptoServiceServer.
func (s *cryptoServer) CreateWallet(ctx context.Context, req *cryptov1.CreateWalletRequest) (*cryptov1.CreateWalletResponse, error) {
	w, err := s.walletSvc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID:      req.WalletId,
		OwnerID: req.OwnerId,
		ChainID: domain.ChainID(req.ChainId),
		Type:    domain.WalletType(req.WalletType),
	})
	if err != nil {
		if errors.Is(err, domain.ErrChainNotSupported) {
			return nil, status.Errorf(codes.InvalidArgument, "unsupported chain: %d", req.ChainId)
		}
		return nil, status.Errorf(codes.Internal, "create wallet: %v", err)
	}
	return &cryptov1.CreateWalletResponse{Wallet: walletToProto(w)}, nil
}

// GetWallet implements CryptoServiceServer.
func (s *cryptoServer) GetWallet(ctx context.Context, req *cryptov1.GetWalletRequest) (*cryptov1.GetWalletResponse, error) {
	w, err := s.walletSvc.GetWallet(ctx, req.WalletId)
	if err != nil {
		if errors.Is(err, domain.ErrWalletNotFound) {
			return nil, status.Errorf(codes.NotFound, "wallet %s not found", req.WalletId)
		}
		return nil, status.Errorf(codes.Internal, "get wallet: %v", err)
	}
	return &cryptov1.GetWalletResponse{Wallet: walletToProto(w)}, nil
}

// ProcessDeposit implements CryptoServiceServer.
func (s *cryptoServer) ProcessDeposit(ctx context.Context, req *cryptov1.ProcessDepositRequest) (*cryptov1.ProcessDepositResponse, error) {
	d, err := s.depositDetector.Detect(ctx, deposits.DetectDepositRequest{
		DepositID:        req.DepositId,
		WalletID:         req.WalletId,
		AccountID:        req.AccountId,
		TxHash:           req.TxHash,
		ChainID:          domain.ChainID(req.ChainId),
		AmountMinorUnits: req.AmountMinorUnits,
	})
	if err != nil {
		return nil, status.Errorf(codes.InvalidArgument, "detect deposit: %v", err)
	}
	d, err = s.depositProc.Process(ctx, d.ID)
	if err != nil {
		if errors.Is(err, domain.ErrNotEnoughConfirmations) {
			return nil, status.Errorf(codes.FailedPrecondition, "not enough confirmations")
		}
		return nil, status.Errorf(codes.Internal, "process deposit: %v", err)
	}
	return &cryptov1.ProcessDepositResponse{Deposit: depositToProto(d)}, nil
}

// RequestWithdrawal implements CryptoServiceServer.
func (s *cryptoServer) RequestWithdrawal(ctx context.Context, req *cryptov1.RequestWithdrawalRequest) (*cryptov1.RequestWithdrawalResponse, error) {
	w, err := s.withdrawalSvc.RequestWithdrawal(ctx, withdrawals.RequestWithdrawalRequest{
		ID:               req.WithdrawalId,
		WalletID:         req.WalletId,
		AccountID:        req.AccountId,
		ToAddress:        req.ToAddress,
		ChainID:          domain.ChainID(req.ChainId),
		AmountMinorUnits: req.AmountMinorUnits,
	})
	if err != nil {
		if errors.Is(err, domain.ErrAmountBelowFee) {
			return nil, status.Errorf(codes.InvalidArgument, "amount is below fee")
		}
		return nil, status.Errorf(codes.Internal, "request withdrawal: %v", err)
	}
	return &cryptov1.RequestWithdrawalResponse{Withdrawal: withdrawalToProto(w)}, nil
}

// ProcessWithdrawal implements CryptoServiceServer.
func (s *cryptoServer) ProcessWithdrawal(ctx context.Context, req *cryptov1.ProcessWithdrawalRequest) (*cryptov1.ProcessWithdrawalResponse, error) {
	w, err := s.withdrawalSvc.ProcessWithdrawal(ctx, req.WithdrawalId)
	if err != nil {
		if errors.Is(err, domain.ErrWithdrawalNotFound) {
			return nil, status.Errorf(codes.NotFound, "withdrawal not found")
		}
		if errors.Is(err, domain.ErrWithdrawalNotPending) {
			return nil, status.Errorf(codes.FailedPrecondition, "withdrawal not pending")
		}
		return nil, status.Errorf(codes.Internal, "process withdrawal: %v", err)
	}
	return &cryptov1.ProcessWithdrawalResponse{Withdrawal: withdrawalToProto(w)}, nil
}

// InternalTransfer implements CryptoServiceServer.
func (s *cryptoServer) InternalTransfer(ctx context.Context, req *cryptov1.InternalTransferRequest) (*cryptov1.InternalTransferResponse, error) {
	tr, err := s.transferSvc.Transfer(ctx, transfers.InternalTransferRequest{
		ID:               req.TransferId,
		FromWalletID:     req.FromWalletId,
		ToWalletID:       req.ToWalletId,
		FromAccountID:    req.FromAccountId,
		ToAccountID:      req.ToAccountId,
		AmountMinorUnits: req.AmountMinorUnits,
	})
	if err != nil {
		if errors.Is(err, domain.ErrChainMismatch) {
			return nil, status.Errorf(codes.InvalidArgument, "wallets are on different chains")
		}
		if errors.Is(err, domain.ErrWalletFrozen) {
			return nil, status.Errorf(codes.PermissionDenied, "wallet is frozen")
		}
		return nil, status.Errorf(codes.Internal, "internal transfer: %v", err)
	}
	return &cryptov1.InternalTransferResponse{Transfer: transferToProto(tr)}, nil
}

// ── proto converters ──────────────────────────────────────────────────────────

func walletToProto(w *domain.Wallet) *cryptov1.WalletProto {
	return &cryptov1.WalletProto{
		WalletId:   w.ID,
		OwnerId:    w.OwnerID,
		ChainId:    int32(w.ChainID),
		Address:    w.Address,
		WalletType: string(w.Type),
		Status:     string(w.Status),
	}
}

func depositToProto(d *domain.Deposit) *cryptov1.DepositProto {
	return &cryptov1.DepositProto{
		DepositId:        d.ID,
		WalletId:         d.WalletID,
		AccountId:        d.AccountID,
		TxHash:           d.TxHash,
		ChainId:          int32(d.ChainID),
		AmountMinorUnits: d.AmountMinorUnits,
		Status:           string(d.Status),
		Confirmations:    int32(d.Confirmations),
	}
}

func withdrawalToProto(w *domain.Withdrawal) *cryptov1.WithdrawalProto {
	return &cryptov1.WithdrawalProto{
		WithdrawalId:     w.ID,
		WalletId:         w.WalletID,
		AccountId:        w.AccountID,
		ToAddress:        w.ToAddress,
		ChainId:          int32(w.ChainID),
		AmountMinorUnits: w.AmountMinorUnits,
		FeeMinorUnits:    w.FeeMinorUnits,
		TxHash:           w.TxHash,
		Status:           string(w.Status),
	}
}

func transferToProto(t *transfers.InternalTransfer) *cryptov1.TransferProto {
	return &cryptov1.TransferProto{
		TransferId:       t.ID,
		FromWalletId:     t.FromWalletID,
		ToWalletId:       t.ToWalletID,
		FromAccountId:    t.FromAccountID,
		ToAccountId:      t.ToAccountID,
		ChainId:          int32(t.ChainID),
		AmountMinorUnits: t.AmountMinorUnits,
	}
}

package transfers_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/engine"
	"github.com/blazil/crypto/internal/transfers"
	"github.com/blazil/crypto/internal/wallets"
)

func newRegistry() *chains.ChainRegistry {
	reg := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		reg.Register(chains.NewMockChainAdapter(c))
	}
	return reg
}

func newWalletSvc() *wallets.InMemoryWalletService {
	return wallets.NewInMemoryWalletService(wallets.NewInMemoryWalletStore(), newRegistry())
}

func seedWallets(t *testing.T, svc *wallets.InMemoryWalletService) {
	t.Helper()
	ctx := context.Background()
	_, err := svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "from-w", OwnerID: "owner-1", ChainID: domain.ChainEthereum, Type: domain.WalletTypeInternal,
	})
	if err != nil {
		t.Fatalf("create from wallet: %v", err)
	}
	_, err = svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "to-w", OwnerID: "owner-2", ChainID: domain.ChainEthereum, Type: domain.WalletTypeInternal,
	})
	if err != nil {
		t.Fatalf("create to wallet: %v", err)
	}
}

func TestInternalTransfer_Success(t *testing.T) {
	walletSvc := newWalletSvc()
	seedWallets(t, walletSvc)
	eng := &engine.MockEngineClient{}
	svc := transfers.NewInMemoryInternalTransferService(walletSvc, eng)
	tr, err := svc.Transfer(context.Background(), transfers.InternalTransferRequest{
		ID:               "tf-1",
		FromWalletID:     "from-w",
		ToWalletID:       "to-w",
		FromAccountID:    "acc-from",
		ToAccountID:      "acc-to",
		AmountMinorUnits: 1000,
	})
	if err != nil {
		t.Fatalf("Transfer: %v", err)
	}
	if tr.AmountMinorUnits != 1000 {
		t.Errorf("expected 1000, got %d", tr.AmountMinorUnits)
	}
}

func TestInternalTransfer_DifferentChains_Rejected(t *testing.T) {
	walletSvc := newWalletSvc()
	ctx := context.Background()
	_, _ = walletSvc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "eth-w", OwnerID: "o1", ChainID: domain.ChainEthereum, Type: domain.WalletTypeInternal,
	})
	_, _ = walletSvc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "btc-w", OwnerID: "o2", ChainID: domain.ChainBitcoin, Type: domain.WalletTypeInternal,
	})
	eng := &engine.MockEngineClient{}
	svc := transfers.NewInMemoryInternalTransferService(walletSvc, eng)
	_, err := svc.Transfer(ctx, transfers.InternalTransferRequest{
		ID: "tf-2", FromWalletID: "eth-w", ToWalletID: "btc-w",
		FromAccountID: "acc-1", ToAccountID: "acc-2", AmountMinorUnits: 5000,
	})
	if !errors.Is(err, domain.ErrChainMismatch) {
		t.Errorf("expected ErrChainMismatch, got %v", err)
	}
}

func TestInternalTransfer_FrozenWallet_Rejected(t *testing.T) {
	walletSvc := newWalletSvc()
	ctx := context.Background()
	_, _ = walletSvc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "frozen-w", OwnerID: "o1", ChainID: domain.ChainEthereum, Type: domain.WalletTypeInternal,
	})
	_, _ = walletSvc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "ok-w", OwnerID: "o2", ChainID: domain.ChainEthereum, Type: domain.WalletTypeInternal,
	})
	_, _ = walletSvc.FreezeWallet(ctx, "frozen-w")
	eng := &engine.MockEngineClient{}
	svc := transfers.NewInMemoryInternalTransferService(walletSvc, eng)
	_, err := svc.Transfer(ctx, transfers.InternalTransferRequest{
		ID: "tf-3", FromWalletID: "frozen-w", ToWalletID: "ok-w",
		FromAccountID: "acc-1", ToAccountID: "acc-2", AmountMinorUnits: 5000,
	})
	if !errors.Is(err, domain.ErrWalletFrozen) {
		t.Errorf("expected ErrWalletFrozen, got %v", err)
	}
}

func TestInternalTransfer_EngineFailure(t *testing.T) {
	walletSvc := newWalletSvc()
	seedWallets(t, walletSvc)
	eng := &engine.MockEngineClient{DebitErr: errors.New("engine unavailable")}
	svc := transfers.NewInMemoryInternalTransferService(walletSvc, eng)
	_, err := svc.Transfer(context.Background(), transfers.InternalTransferRequest{
		ID: "tf-4", FromWalletID: "from-w", ToWalletID: "to-w",
		FromAccountID: "acc-from", ToAccountID: "acc-to", AmountMinorUnits: 500,
	})
	if err == nil {
		t.Fatal("expected error from engine debit failure")
	}
}

func TestInternalTransfer_ZeroAmount_Rejected(t *testing.T) {
	walletSvc := newWalletSvc()
	seedWallets(t, walletSvc)
	eng := &engine.MockEngineClient{}
	svc := transfers.NewInMemoryInternalTransferService(walletSvc, eng)
	_, err := svc.Transfer(context.Background(), transfers.InternalTransferRequest{
		ID: "tf-5", FromWalletID: "from-w", ToWalletID: "to-w",
		FromAccountID: "acc-from", ToAccountID: "acc-to", AmountMinorUnits: 0,
	})
	if !errors.Is(err, domain.ErrInsufficientAmount) {
		t.Errorf("expected ErrInsufficientAmount, got %v", err)
	}
}

package wallets_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/crypto/internal/chains"
	"github.com/blazil/crypto/internal/domain"
	"github.com/blazil/crypto/internal/wallets"
)

func newWalletSvc() *wallets.InMemoryWalletService {
	registry := chains.NewChainRegistry()
	for _, c := range domain.SupportedChains() {
		registry.Register(chains.NewMockChainAdapter(c))
	}
	return wallets.NewInMemoryWalletService(wallets.NewInMemoryWalletStore(), registry)
}

func TestCreateWallet_Success(t *testing.T) {
	svc := newWalletSvc()
	w, err := svc.CreateWallet(context.Background(), wallets.CreateWalletRequest{
		ID:      "w1",
		OwnerID: "owner-abc",
		ChainID: domain.ChainEthereum,
		Type:    domain.WalletTypeDeposit,
	})
	if err != nil {
		t.Fatalf("CreateWallet: %v", err)
	}
	if w.Address == "" {
		t.Error("expected non-empty address")
	}
	if w.Status != domain.WalletStatusActive {
		t.Errorf("expected active, got %s", w.Status)
	}
	// Address format: mock_ETH_{first 8 chars of ownerID}
	expected := "mock_ETH_owner-ab"
	if w.Address != expected {
		t.Errorf("address: want %q, got %q", expected, w.Address)
	}
}

func TestCreateWallet_UnsupportedChain(t *testing.T) {
	svc := newWalletSvc()
	_, err := svc.CreateWallet(context.Background(), wallets.CreateWalletRequest{
		ID:      "w2",
		OwnerID: "owner-1",
		ChainID: domain.ChainID(99),
		Type:    domain.WalletTypeDeposit,
	})
	if !errors.Is(err, domain.ErrChainNotSupported) {
		t.Errorf("expected ErrChainNotSupported, got %v", err)
	}
}

func TestGetWallet_NotFound(t *testing.T) {
	svc := newWalletSvc()
	_, err := svc.GetWallet(context.Background(), "does-not-exist")
	if !errors.Is(err, domain.ErrWalletNotFound) {
		t.Errorf("expected ErrWalletNotFound, got %v", err)
	}
}

func TestFreezeWallet(t *testing.T) {
	svc := newWalletSvc()
	ctx := context.Background()
	_, _ = svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "w3", OwnerID: "o1", ChainID: domain.ChainBitcoin, Type: domain.WalletTypeDeposit,
	})
	w, err := svc.FreezeWallet(ctx, "w3")
	if err != nil {
		t.Fatalf("FreezeWallet: %v", err)
	}
	if w.Status != domain.WalletStatusFrozen {
		t.Errorf("expected frozen, got %s", w.Status)
	}
}

func TestArchiveWallet(t *testing.T) {
	svc := newWalletSvc()
	ctx := context.Background()
	_, _ = svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "w4", OwnerID: "o1", ChainID: domain.ChainSolana, Type: domain.WalletTypeWithdraw,
	})
	w, err := svc.ArchiveWallet(ctx, "w4")
	if err != nil {
		t.Fatalf("ArchiveWallet: %v", err)
	}
	if w.Status != domain.WalletStatusArchived {
		t.Errorf("expected archived, got %s", w.Status)
	}
}

func TestListWalletsByOwner(t *testing.T) {
	svc := newWalletSvc()
	ctx := context.Background()
	_, _ = svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "w5", OwnerID: "ownerX", ChainID: domain.ChainEthereum, Type: domain.WalletTypeDeposit,
	})
	_, _ = svc.CreateWallet(ctx, wallets.CreateWalletRequest{
		ID: "w6", OwnerID: "ownerX", ChainID: domain.ChainPolygon, Type: domain.WalletTypeWithdraw,
	})
	ws, err := svc.ListWalletsByOwner(ctx, "ownerX")
	if err != nil {
		t.Fatalf("ListWalletsByOwner: %v", err)
	}
	if len(ws) != 2 {
		t.Errorf("expected 2 wallets, got %d", len(ws))
	}
}

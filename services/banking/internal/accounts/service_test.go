package accounts_test

import (
	"context"
	"errors"
	"testing"

	"github.com/blazil/banking/internal/accounts"
	"github.com/blazil/banking/internal/domain"
)

func newService() *accounts.InMemoryAccountService {
	return accounts.NewInMemoryAccountService()
}

func createReq(id, owner string) accounts.CreateAccountRequest {
	return accounts.CreateAccountRequest{
		ID:                       domain.AccountID(id),
		OwnerID:                  owner,
		Type:                     domain.AccountTypeChecking,
		CurrencyCode:             "USD",
		InitialBalanceMinorUnits: 10_000, // $100.00
	}
}

func TestCreateAccount_Success(t *testing.T) {
	svc := newService()
	acc, err := svc.CreateAccount(context.Background(), createReq("acc-1", "owner-1"))
	if err != nil {
		t.Fatalf("CreateAccount: %v", err)
	}
	if acc.ID != "acc-1" {
		t.Errorf("ID: got %s, want acc-1", acc.ID)
	}
	if acc.BalanceMinorUnits != 10_000 {
		t.Errorf("Balance: got %d, want 10000", acc.BalanceMinorUnits)
	}
	if acc.Status != domain.AccountStatusActive {
		t.Errorf("Status: got %s, want active", acc.Status)
	}
}

func TestCreateAccount_DuplicateID(t *testing.T) {
	svc := newService()
	_, _ = svc.CreateAccount(context.Background(), createReq("acc-dup", "owner-1"))
	_, err := svc.CreateAccount(context.Background(), createReq("acc-dup", "owner-2"))
	if !errors.Is(err, domain.ErrAccountAlreadyExists) {
		t.Errorf("expected ErrAccountAlreadyExists, got %v", err)
	}
}

func TestGetAccount_NotFound(t *testing.T) {
	svc := newService()
	_, err := svc.GetAccount(context.Background(), "missing")
	if !errors.Is(err, domain.ErrAccountNotFound) {
		t.Errorf("expected ErrAccountNotFound, got %v", err)
	}
}

func TestListAccountsByOwner(t *testing.T) {
	svc := newService()
	ctx := context.Background()
	_, _ = svc.CreateAccount(ctx, createReq("a1", "alice"))
	_, _ = svc.CreateAccount(ctx, createReq("a2", "alice"))
	_, _ = svc.CreateAccount(ctx, createReq("a3", "bob"))

	accs, err := svc.ListAccountsByOwner(ctx, "alice")
	if err != nil {
		t.Fatalf("ListAccountsByOwner: %v", err)
	}
	if len(accs) != 2 {
		t.Errorf("expected 2 accounts for alice, got %d", len(accs))
	}
}

func TestFreezeAccount(t *testing.T) {
	svc := newService()
	ctx := context.Background()
	_, _ = svc.CreateAccount(ctx, createReq("acc-frz", "owner-frz"))

	if err := svc.FreezeAccount(ctx, "acc-frz"); err != nil {
		t.Fatalf("FreezeAccount: %v", err)
	}
	acc, _ := svc.GetAccount(ctx, "acc-frz")
	if acc.Status != domain.AccountStatusFrozen {
		t.Errorf("expected frozen, got %s", acc.Status)
	}

	// Freezing again should return ErrAccountFrozen.
	if err := svc.FreezeAccount(ctx, "acc-frz"); !errors.Is(err, domain.ErrAccountFrozen) {
		t.Errorf("expected ErrAccountFrozen, got %v", err)
	}
}

func TestCloseAccount_WithoutBalanceChecker(t *testing.T) {
	// Without a BalanceChecker set, CloseAccount should succeed (no balance check).
	svc := newService()
	ctx := context.Background()
	_, _ = svc.CreateAccount(ctx, createReq("acc-cls", "owner-cls"))

	if err := svc.CloseAccount(ctx, "acc-cls"); err != nil {
		t.Fatalf("CloseAccount: %v", err)
	}
	acc, _ := svc.GetAccount(ctx, "acc-cls")
	if acc.Status != domain.AccountStatusClosed {
		t.Errorf("expected closed, got %s", acc.Status)
	}

	// Closing again should return ErrAccountClosed.
	if err := svc.CloseAccount(ctx, "acc-cls"); !errors.Is(err, domain.ErrAccountClosed) {
		t.Errorf("expected ErrAccountClosed on second close, got %v", err)
	}
}

func TestCloseAccount_WithNonZeroBalance_Rejected(t *testing.T) {
	svc := newService()
	ctx := context.Background()
	_, _ = svc.CreateAccount(ctx, createReq("acc-bal-cls", "owner-x"))

	// Inject a stub BalanceChecker that reports non-zero balance.
	svc.SetBalanceService(&stubBalanceChecker{minorUnits: 5_000, currency: "USD"})

	err := svc.CloseAccount(ctx, "acc-bal-cls")
	if !errors.Is(err, domain.ErrAccountHasBalance) {
		t.Errorf("expected ErrAccountHasBalance, got %v", err)
	}
}

// stubBalanceChecker satisfies accounts.BalanceChecker for testing.
type stubBalanceChecker struct {
	minorUnits int64
	currency   string
}

func (s *stubBalanceChecker) GetBalance(_ context.Context, id domain.AccountID) (*domain.Balance, error) {
	return &domain.Balance{AccountID: id, MinorUnits: s.minorUnits, CurrencyCode: s.currency}, nil
}

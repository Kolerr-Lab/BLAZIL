package balances_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/blazil/banking/internal/accounts"
	"github.com/blazil/banking/internal/balances"
	"github.com/blazil/banking/internal/domain"
	"github.com/blazil/banking/internal/history"
)

// setupBalance creates an account + balance store + BalanceService pre-seeded with
// one active checking account with a $500.00 (50000 cents) opening balance.
func setupBalance(t *testing.T) (*accounts.InMemoryAccountService, *balances.AccountBalanceService) {
	t.Helper()
	accSvc := accounts.NewInMemoryAccountService()
	balStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()

	ctx := context.Background()
	_, err := accSvc.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID:                       "acc-bal-1",
		OwnerID:                  "owner-1",
		Type:                     domain.AccountTypeChecking,
		CurrencyCode:             "USD",
		InitialBalanceMinorUnits: 50_000,
	})
	if err != nil {
		t.Fatalf("setup CreateAccount: %v", err)
	}
	// Seed the balance store with the opening balance.
	_ = balStore.Set(ctx, &domain.Balance{
		AccountID:    "acc-bal-1",
		MinorUnits:   50_000,
		CurrencyCode: "USD",
		UpdatedAt:    time.Now().UTC(),
	})

	balSvc := balances.NewAccountBalanceService(accSvc, balStore, txStore)
	return accSvc, balSvc
}

func TestGetBalance_Success(t *testing.T) {
	_, balSvc := setupBalance(t)
	bal, err := balSvc.GetBalance(context.Background(), "acc-bal-1")
	if err != nil {
		t.Fatalf("GetBalance: %v", err)
	}
	if bal.MinorUnits != 50_000 {
		t.Errorf("expected 50000, got %d", bal.MinorUnits)
	}
}

func TestGetBalance_NotFound(t *testing.T) {
	_, balSvc := setupBalance(t)
	_, err := balSvc.GetBalance(context.Background(), "missing")
	if !errors.Is(err, domain.ErrAccountNotFound) {
		t.Errorf("expected ErrAccountNotFound, got %v", err)
	}
}

func TestGetBalanceSummary_Fields(t *testing.T) {
	_, balSvc := setupBalance(t)
	summary, err := balSvc.GetBalanceSummary(context.Background(), "acc-bal-1")
	if err != nil {
		t.Fatalf("GetBalanceSummary: %v", err)
	}
	if summary.CurrencyCode != "USD" {
		t.Errorf("CurrencyCode: got %s, want USD", summary.CurrencyCode)
	}
	if summary.Status != domain.AccountStatusActive {
		t.Errorf("Status: got %s, want active", summary.Status)
	}
	if summary.BalanceMinorUnits != 50_000 {
		t.Errorf("Balance: got %d, want 50000", summary.BalanceMinorUnits)
	}
}

func TestBalanceService_CreditAppendsTransaction(t *testing.T) {
	accSvc := accounts.NewInMemoryAccountService()
	balStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()
	ctx := context.Background()

	_, _ = accSvc.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID: "acc-c1", OwnerID: "o1", Type: domain.AccountTypeChecking, CurrencyCode: "USD",
	})
	_ = balStore.Set(ctx, &domain.Balance{AccountID: "acc-c1", MinorUnits: 0, CurrencyCode: "USD", UpdatedAt: time.Now().UTC()})
	balSvc := balances.NewAccountBalanceService(accSvc, balStore, txStore)

	newBal, err := balSvc.Credit(ctx, "acc-c1", 10_000, "USD", "deposit", "pay-1")
	if err != nil {
		t.Fatalf("Credit: %v", err)
	}
	if newBal.MinorUnits != 10_000 {
		t.Errorf("balance after credit: got %d, want 10000", newBal.MinorUnits)
	}

	txs, err := txStore.ListByAccount(ctx, "acc-c1", history.ListOptions{})
	if err != nil {
		t.Fatalf("ListByAccount: %v", err)
	}
	if len(txs) != 1 {
		t.Fatalf("expected 1 transaction, got %d", len(txs))
	}
	if txs[0].Type != domain.TransactionTypeCredit {
		t.Errorf("tx type: got %s, want credit", txs[0].Type)
	}
	if txs[0].AmountMinorUnits != 10_000 {
		t.Errorf("tx amount: got %d, want 10000", txs[0].AmountMinorUnits)
	}
}

func TestBalanceService_DebitAppendsTransaction(t *testing.T) {
	accSvc := accounts.NewInMemoryAccountService()
	balStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()
	ctx := context.Background()

	_, _ = accSvc.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID: "acc-d1", OwnerID: "o2", Type: domain.AccountTypeChecking, CurrencyCode: "USD",
	})
	_ = balStore.Set(ctx, &domain.Balance{AccountID: "acc-d1", MinorUnits: 20_000, CurrencyCode: "USD", UpdatedAt: time.Now().UTC()})
	balSvc := balances.NewAccountBalanceService(accSvc, balStore, txStore)

	newBal, err := balSvc.Debit(ctx, "acc-d1", 5_000, "USD", "withdrawal", "pay-2")
	if err != nil {
		t.Fatalf("Debit: %v", err)
	}
	if newBal.MinorUnits != 15_000 {
		t.Errorf("balance after debit: got %d, want 15000", newBal.MinorUnits)
	}

	txs, err := txStore.ListByAccount(ctx, "acc-d1", history.ListOptions{})
	if err != nil {
		t.Fatalf("ListByAccount: %v", err)
	}
	if len(txs) != 1 {
		t.Fatalf("expected 1 transaction, got %d", len(txs))
	}
	if txs[0].Type != domain.TransactionTypeDebit {
		t.Errorf("tx type: got %s, want debit", txs[0].Type)
	}
}

func TestBalanceService_DebitFrozenAccount_Rejected(t *testing.T) {
	accSvc := accounts.NewInMemoryAccountService()
	balStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()
	ctx := context.Background()

	_, _ = accSvc.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID: "acc-f1", OwnerID: "o3", Type: domain.AccountTypeChecking, CurrencyCode: "USD",
	})
	_ = balStore.Set(ctx, &domain.Balance{AccountID: "acc-f1", MinorUnits: 10_000, CurrencyCode: "USD", UpdatedAt: time.Now().UTC()})
	_ = accSvc.FreezeAccount(ctx, "acc-f1")

	balSvc := balances.NewAccountBalanceService(accSvc, balStore, txStore)

	_, err := balSvc.Debit(ctx, "acc-f1", 1_000, "USD", "test", "pay-3")
	if !errors.Is(err, domain.ErrAccountFrozen) {
		t.Errorf("expected ErrAccountFrozen, got %v", err)
	}
}

func TestBalanceService_CreditClosedAccount_Rejected(t *testing.T) {
	accSvc := accounts.NewInMemoryAccountService()
	balStore := balances.NewInMemoryBalanceStore()
	txStore := history.NewInMemoryTransactionStore()
	ctx := context.Background()

	_, _ = accSvc.CreateAccount(ctx, accounts.CreateAccountRequest{
		ID: "acc-cl1", OwnerID: "o4", Type: domain.AccountTypeChecking, CurrencyCode: "USD",
	})
	_ = balStore.Set(ctx, &domain.Balance{AccountID: "acc-cl1", MinorUnits: 0, CurrencyCode: "USD", UpdatedAt: time.Now().UTC()})
	_ = accSvc.CloseAccount(ctx, "acc-cl1")

	balSvc := balances.NewAccountBalanceService(accSvc, balStore, txStore)

	_, err := balSvc.Credit(ctx, "acc-cl1", 1_000, "USD", "test", "pay-4")
	if !errors.Is(err, domain.ErrAccountClosed) {
		t.Errorf("expected ErrAccountClosed, got %v", err)
	}
}

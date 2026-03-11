package history_test

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"

	"github.com/blazil/banking/internal/domain"
	"github.com/blazil/banking/internal/history"
)

func makeTx(id, accountID string, ts time.Time) *domain.Transaction {
	return &domain.Transaction{
		ID:               domain.TransactionID(id),
		AccountID:        domain.AccountID(accountID),
		Type:             domain.TransactionTypeCredit,
		AmountMinorUnits: 1_000,
		CurrencyCode:     "USD",
		Timestamp:        ts,
	}
}

func TestAppend_GetByID(t *testing.T) {
	store := history.NewInMemoryTransactionStore()
	ctx := context.Background()
	tx := makeTx("tx-1", "acc-1", time.Now())

	if err := store.Append(ctx, tx); err != nil {
		t.Fatalf("Append: %v", err)
	}

	got, err := store.GetByID(ctx, "tx-1")
	if err != nil {
		t.Fatalf("GetByID: %v", err)
	}
	if got.ID != "tx-1" {
		t.Errorf("ID: got %s, want tx-1", got.ID)
	}
}

func TestGetByID_NotFound(t *testing.T) {
	store := history.NewInMemoryTransactionStore()
	_, err := store.GetByID(context.Background(), "missing")
	if !errors.Is(err, domain.ErrTransactionNotFound) {
		t.Errorf("expected ErrTransactionNotFound, got %v", err)
	}
}

func TestListByAccount_OrderedNewestFirst(t *testing.T) {
	store := history.NewInMemoryTransactionStore()
	ctx := context.Background()
	now := time.Now()

	_ = store.Append(ctx, makeTx("tx-old", "acc-a", now.Add(-2*time.Hour)))
	_ = store.Append(ctx, makeTx("tx-mid", "acc-a", now.Add(-1*time.Hour)))
	_ = store.Append(ctx, makeTx("tx-new", "acc-a", now))

	txs, err := store.ListByAccount(ctx, "acc-a", history.ListOptions{})
	if err != nil {
		t.Fatalf("ListByAccount: %v", err)
	}
	if len(txs) != 3 {
		t.Fatalf("expected 3 transactions, got %d", len(txs))
	}
	if txs[0].ID != "tx-new" {
		t.Errorf("first should be newest, got %s", txs[0].ID)
	}
}

func TestListByAccount_Pagination(t *testing.T) {
	store := history.NewInMemoryTransactionStore()
	ctx := context.Background()
	now := time.Now()

	for i := 0; i < 5; i++ {
		txID := fmt.Sprintf("%c-tx", 'a'+i)
		_ = store.Append(ctx, makeTx(
			txID,
			"acc-p",
			now.Add(time.Duration(i)*time.Minute),
		))
	}

	page, err := store.ListByAccount(ctx, "acc-p", history.ListOptions{Limit: 2, Offset: 1})
	if err != nil {
		t.Fatalf("ListByAccount paginated: %v", err)
	}
	if len(page) != 2 {
		t.Errorf("expected 2 results, got %d", len(page))
	}
}

func TestListByAccount_EmptyForUnknownAccount(t *testing.T) {
	store := history.NewInMemoryTransactionStore()
	txs, err := store.ListByAccount(context.Background(), "no-such-acc", history.ListOptions{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(txs) != 0 {
		t.Errorf("expected empty, got %d", len(txs))
	}
}

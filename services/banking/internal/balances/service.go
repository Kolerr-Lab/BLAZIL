// Package balances implements balance management for the Blazil banking service.
package balances

import (
	"context"
	"fmt"
	"time"

	"github.com/google/uuid"

	"github.com/blazil/banking/internal/accounts"
	"github.com/blazil/banking/internal/domain"
	"github.com/blazil/banking/internal/history"
	"github.com/blazil/observability"
)

// BalanceSummary is a rich snapshot of an account's current balance.
type BalanceSummary struct {
	AccountID         domain.AccountID
	BalanceMinorUnits int64
	CurrencyCode      string
	Status            domain.AccountStatus
}

// BalanceService manages balance reads and mutations for bank accounts.
// All implementations must be safe for concurrent use.
type BalanceService interface {
	// GetBalance returns the canonical Balance for the given account.
	GetBalance(ctx context.Context, id domain.AccountID) (*domain.Balance, error)

	// GetBalanceSummary returns a rich snapshot combining account metadata + balance.
	GetBalanceSummary(ctx context.Context, id domain.AccountID) (*BalanceSummary, error)

	// Credit adds amountMinorUnits to the account balance.
	// The account must be Active. Appends a Transaction record.
	Credit(ctx context.Context, id domain.AccountID, amountMinorUnits int64, currencyCode, reference, paymentID string) (*domain.Balance, error)

	// Debit subtracts amountMinorUnits from the account balance.
	// The account must be Active. Sufficient funds required for non-loan accounts.
	// Appends a Transaction record.
	Debit(ctx context.Context, id domain.AccountID, amountMinorUnits int64, currencyCode, reference, paymentID string) (*domain.Balance, error)
}

// AccountBalanceService implements BalanceService and accounts.BalanceChecker.
type AccountBalanceService struct {
	accounts accounts.AccountService
	store    BalanceStore
	history  history.TransactionStore
}

// NewAccountBalanceService constructs an AccountBalanceService.
func NewAccountBalanceService(
	svc accounts.AccountService,
	store BalanceStore,
	txStore history.TransactionStore,
) *AccountBalanceService {
	return &AccountBalanceService{accounts: svc, store: store, history: txStore}
}

// GetBalance implements BalanceService and accounts.BalanceChecker.
func (s *AccountBalanceService) GetBalance(ctx context.Context, id domain.AccountID) (*domain.Balance, error) {
	bal, err := s.store.Get(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("GetBalance: %w", err)
	}
	return bal, nil
}

// GetBalanceSummary implements BalanceService.
func (s *AccountBalanceService) GetBalanceSummary(ctx context.Context, id domain.AccountID) (*BalanceSummary, error) {
	acc, err := s.accounts.GetAccount(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("GetBalanceSummary: %w", err)
	}
	bal, err := s.store.Get(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("GetBalanceSummary: %w", err)
	}
	return &BalanceSummary{
		AccountID:         acc.ID,
		BalanceMinorUnits: bal.MinorUnits,
		CurrencyCode:      bal.CurrencyCode,
		Status:            acc.Status,
	}, nil
}

// Credit implements BalanceService.
func (s *AccountBalanceService) Credit(ctx context.Context, id domain.AccountID, amountMinorUnits int64, currencyCode, reference, paymentID string) (*domain.Balance, error) {
	start := time.Now()
	if amountMinorUnits < 0 {
		return nil, domain.ErrNegativeAmount
	}
	acc, err := s.accounts.GetAccount(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("Credit: %w", err)
	}
	if acc.Status != domain.AccountStatusActive {
		return nil, accountStatusErr(acc.Status)
	}
	bal, err := s.store.Get(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("Credit: %w", err)
	}
	newBal := &domain.Balance{
		AccountID:    id,
		MinorUnits:   bal.MinorUnits + amountMinorUnits,
		CurrencyCode: bal.CurrencyCode,
		UpdatedAt:    time.Now().UTC(),
	}
	if err := s.store.Set(ctx, newBal); err != nil {
		return nil, fmt.Errorf("Credit store: %w", err)
	}
	tx := &domain.Transaction{
		ID:                     domain.TransactionID(uuid.New().String()),
		AccountID:              id,
		Type:                   domain.TransactionTypeCredit,
		AmountMinorUnits:       amountMinorUnits,
		CurrencyCode:           currencyCode,
		BalanceAfterMinorUnits: newBal.MinorUnits,
		Description:            reference,
		Reference:              paymentID,
		Timestamp:              newBal.UpdatedAt,
	}
	if err := s.history.Append(ctx, tx); err != nil {
		return nil, fmt.Errorf("Credit history: %w", err)
	}
	observability.TransactionsTotal.WithLabelValues("banking", "success", "internal").Inc()
	observability.TransactionDuration.WithLabelValues("banking", "credit").Observe(time.Since(start).Seconds())
	return newBal, nil
}

// Debit implements BalanceService.
func (s *AccountBalanceService) Debit(ctx context.Context, id domain.AccountID, amountMinorUnits int64, currencyCode, reference, paymentID string) (*domain.Balance, error) {
	start := time.Now()
	if amountMinorUnits < 0 {
		return nil, domain.ErrNegativeAmount
	}
	acc, err := s.accounts.GetAccount(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("Debit: %w", err)
	}
	if acc.Status != domain.AccountStatusActive {
		return nil, accountStatusErr(acc.Status)
	}
	bal, err := s.store.Get(ctx, id)
	if err != nil {
		return nil, fmt.Errorf("Debit: %w", err)
	}
	if acc.Type != domain.AccountTypeLoan && bal.MinorUnits < amountMinorUnits {
		return nil, domain.ErrInsufficientFunds
	}
	newBal := &domain.Balance{
		AccountID:    id,
		MinorUnits:   bal.MinorUnits - amountMinorUnits,
		CurrencyCode: bal.CurrencyCode,
		UpdatedAt:    time.Now().UTC(),
	}
	if err := s.store.Set(ctx, newBal); err != nil {
		return nil, fmt.Errorf("Debit store: %w", err)
	}
	tx := &domain.Transaction{
		ID:                     domain.TransactionID(uuid.New().String()),
		AccountID:              id,
		Type:                   domain.TransactionTypeDebit,
		AmountMinorUnits:       amountMinorUnits,
		CurrencyCode:           currencyCode,
		BalanceAfterMinorUnits: newBal.MinorUnits,
		Description:            reference,
		Reference:              paymentID,
		Timestamp:              newBal.UpdatedAt,
	}
	if err := s.history.Append(ctx, tx); err != nil {
		return nil, fmt.Errorf("Debit history: %w", err)
	}
	observability.TransactionsTotal.WithLabelValues("banking", "success", "internal").Inc()
	observability.TransactionDuration.WithLabelValues("banking", "debit").Observe(time.Since(start).Seconds())
	return newBal, nil
}

func accountStatusErr(status domain.AccountStatus) error {
	switch status {
	case domain.AccountStatusClosed:
		return domain.ErrAccountClosed
	case domain.AccountStatusFrozen:
		return domain.ErrAccountFrozen
	default:
		return domain.ErrAccountNotActive
	}
}

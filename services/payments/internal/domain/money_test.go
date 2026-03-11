package domain_test

import (
	"testing"

	"github.com/blazil/services/payments/internal/domain"
)

func TestNewMoney(t *testing.T) {
	m := domain.NewMoney(1050, domain.USD)
	if m.MinorUnits != 1050 {
		t.Errorf("MinorUnits: got %d, want 1050", m.MinorUnits)
	}
	if m.Currency.Code != "USD" {
		t.Errorf("Currency: got %q, want USD", m.Currency.Code)
	}
}

func TestMoneyAdd_SameCurrency(t *testing.T) {
	a := domain.NewMoney(500, domain.USD)
	b := domain.NewMoney(300, domain.USD)
	result, err := a.Add(b)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.MinorUnits != 800 {
		t.Errorf("sum: got %d, want 800", result.MinorUnits)
	}
}

func TestMoneyAdd_CurrencyMismatch_Error(t *testing.T) {
	a := domain.NewMoney(500, domain.USD)
	b := domain.NewMoney(300, domain.EUR)
	_, err := a.Add(b)
	if err == nil {
		t.Fatal("expected error for currency mismatch, got nil")
	}
}

func TestMoneyIsZero(t *testing.T) {
	if !domain.NewMoney(0, domain.USD).IsZero() {
		t.Error("expected IsZero() == true")
	}
	if domain.NewMoney(1, domain.USD).IsZero() {
		t.Error("expected IsZero() == false for non-zero amount")
	}
}

func TestMoneyIsNegative(t *testing.T) {
	if !domain.NewMoney(-1, domain.USD).IsNegative() {
		t.Error("expected IsNegative() == true")
	}
	if domain.NewMoney(0, domain.USD).IsNegative() {
		t.Error("expected IsNegative() == false for zero")
	}
	if domain.NewMoney(100, domain.USD).IsNegative() {
		t.Error("expected IsNegative() == false for positive")
	}
}

func TestMoneyString_USD(t *testing.T) {
	m := domain.NewMoney(1050, domain.USD) // $10.50
	want := "10.50 USD"
	if m.String() != want {
		t.Errorf("String(): got %q, want %q", m.String(), want)
	}
}

func TestMoneyString_BTC(t *testing.T) {
	// 1 satoshi = 0.00000001 BTC
	m := domain.NewMoney(1, domain.BTC)
	want := "0.00000001 BTC"
	if m.String() != want {
		t.Errorf("String(): got %q, want %q", m.String(), want)
	}
}

func TestMoneyString_VND(t *testing.T) {
	m := domain.NewMoney(1000, domain.VND)
	want := "1000 VND"
	if m.String() != want {
		t.Errorf("String(): got %q, want %q", m.String(), want)
	}
}

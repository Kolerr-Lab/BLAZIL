package lifecycle_test

import (
	"errors"
	"testing"

	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/lifecycle"
)

func makePayment(id, key string) *domain.Payment {
	return &domain.Payment{
		ID:             domain.PaymentID(id),
		IdempotencyKey: key,
		Status:         domain.StatusPending,
	}
}

func TestPaymentStore_SaveAndGetByID(t *testing.T) {
	s := lifecycle.NewInMemoryPaymentStore()
	p := makePayment("pay-1", "key-1")

	if err := s.Save(p); err != nil {
		t.Fatalf("Save: %v", err)
	}

	got, err := s.GetByID(p.ID)
	if err != nil {
		t.Fatalf("GetByID: %v", err)
	}
	if got.ID != p.ID {
		t.Errorf("got ID %s, want %s", got.ID, p.ID)
	}
}

func TestPaymentStore_GetByIdempotencyKey(t *testing.T) {
	s := lifecycle.NewInMemoryPaymentStore()
	p := makePayment("pay-2", "key-2")

	// Miss before save.
	got, err := s.GetByIdempotencyKey("key-2")
	if err != nil || got != nil {
		t.Fatalf("expected nil,nil before save; got %v, %v", got, err)
	}

	if err := s.Save(p); err != nil {
		t.Fatalf("Save: %v", err)
	}

	got, err = s.GetByIdempotencyKey("key-2")
	if err != nil {
		t.Fatalf("GetByIdempotencyKey: %v", err)
	}
	if got == nil || got.ID != p.ID {
		t.Errorf("expected payment pay-2, got %v", got)
	}
}

func TestPaymentStore_GetByID_NotFound(t *testing.T) {
	s := lifecycle.NewInMemoryPaymentStore()

	_, err := s.GetByID("nonexistent")
	if err == nil {
		t.Fatal("expected error for missing ID, got nil")
	}
	if !errors.Is(err, domain.ErrPaymentNotFound) {
		t.Errorf("expected ErrPaymentNotFound, got %v", err)
	}
}

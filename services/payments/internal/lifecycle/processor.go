// Package lifecycle orchestrates the payment processing lifecycle.
package lifecycle

import (
	"context"
	"fmt"
	"time"

	"github.com/google/uuid"

	"github.com/blazil/observability"
	"github.com/blazil/services/payments/internal/authorization"
	"github.com/blazil/services/payments/internal/domain"
	"github.com/blazil/services/payments/internal/engine"
	"github.com/blazil/services/payments/internal/routing"
)

// PaymentProcessor orchestrates the full payment lifecycle:
// idempotency → authorization → routing → engine submission.
type PaymentProcessor struct {
	store        PaymentStore
	authorizer   authorization.Authorizer
	router       routing.PaymentRouter
	idempotency  IdempotencyStore
	engineClient engine.BlazerEngineClient
}

// NewPaymentProcessor constructs a PaymentProcessor with the provided dependencies.
func NewPaymentProcessor(
	store PaymentStore,
	auth authorization.Authorizer,
	router routing.PaymentRouter,
	idempotency IdempotencyStore,
	engineClient engine.BlazerEngineClient,
) *PaymentProcessor {
	return &PaymentProcessor{
		store:        store,
		authorizer:   auth,
		router:       router,
		idempotency:  idempotency,
		engineClient: engineClient,
	}
}

// Process runs the full payment lifecycle for the given request.
//
// Returns the resulting Payment (which may have StatusFailed for authorization
// rejections) and a non-nil error only for infrastructure failures.
func (p *PaymentProcessor) Process(ctx context.Context, req domain.ProcessPaymentRequest) (*domain.Payment, error) {
	start := time.Now()

	// STEP 1 — Idempotency check: return cached result without reprocessing.
	if existing := p.idempotency.Get(req.IdempotencyKey); existing != nil {
		return existing, nil
	}

	// STEP 2 — Build the Payment struct.
	now := time.Now().UTC()
	payment := &domain.Payment{
		ID:              domain.PaymentID(uuid.New().String()),
		IdempotencyKey:  req.IdempotencyKey,
		DebitAccountID:  req.DebitAccountID,
		CreditAccountID: req.CreditAccountID,
		Amount:          req.Amount,
		LedgerID:        req.LedgerID,
		Status:          domain.StatusPending,
		CreatedAt:       now,
		UpdatedAt:       now,
		Metadata:        req.Metadata,
	}

	// STEP 3 — Authorization.
	result := p.authorizer.Authorize(ctx, payment)
	if !result.Approved {
		payment.Status = domain.StatusFailed
		payment.FailureReason = result.Reason
		payment.UpdatedAt = time.Now().UTC()
		p.idempotency.Set(req.IdempotencyKey, payment)
		if err := p.store.Save(payment); err != nil {
			return nil, fmt.Errorf("store failed for payment %s: %w", payment.ID, err)
		}
		return payment, nil
	}
	payment.Status = domain.StatusAuthorized
	payment.UpdatedAt = time.Now().UTC()

	// STEP 4 — Routing.
	rails, err := p.router.Route(ctx, payment)
	if err != nil {
		return nil, fmt.Errorf("routing failed for payment %s: %w", payment.ID, err)
	}
	payment.Rails = rails

	// STEP 5 — Engine submission (internal rails only).
	if payment.Rails == domain.RailsInternal {
		committed, transferID, err := p.engineClient.Submit(ctx, payment)
		if err != nil {
			return nil, fmt.Errorf("engine submission failed for payment %s: %w", payment.ID, err)
		}
		if committed {
			payment.Status = domain.StatusSettled
			_ = transferID
		} else {
			payment.Status = domain.StatusFailed
			payment.FailureReason = "engine rejected"
		}
	} else {
		// External rails: cleared now, settled asynchronously in a future prompt.
		payment.Status = domain.StatusCleared
	}
	payment.UpdatedAt = time.Now().UTC()

	// STEP 6 — Persist idempotency record and payment store.
	p.idempotency.Set(req.IdempotencyKey, payment)
	if err := p.store.Save(payment); err != nil {
		return nil, fmt.Errorf("store failed for payment %s: %w", payment.ID, err)
	}

	// STEP 7 — Record metrics and return.
	observability.TransactionsTotal.WithLabelValues("payments", payment.Status.String(), payment.Rails.String()).Inc()
	observability.TransactionDuration.WithLabelValues("payments", "process").Observe(time.Since(start).Seconds())
	return payment, nil
}

// GetPayment retrieves a previously processed payment by its ID.
// Returns (nil, ErrPaymentNotFound) when the ID is unknown.
func (p *PaymentProcessor) GetPayment(id domain.PaymentID) (*domain.Payment, error) {
	return p.store.GetByID(id)
}



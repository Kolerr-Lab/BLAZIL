package billing

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/stripe/stripe-go/v82"
	"github.com/stripe/stripe-go/v82/customer"
	"github.com/stripe/stripe-go/v82/webhook"
	"go.uber.org/zap"
)

// StripeHandler validates Stripe webhook events and drives invoice lifecycle.
//
// Security notes:
//   - Every request is validated with webhook.ConstructEvent before any data is read.
//     An invalid Stripe-Signature returns 400 immediately.
//   - Body is capped at 64 KiB to prevent memory exhaustion DoS.
//   - The raw API key (STRIPE_SECRET_KEY) is set globally via stripe.Key — never
//     logged, never exposed in JSON responses.
type StripeHandler struct {
	store         Store
	webhookSecret string // STRIPE_WEBHOOK_SECRET (whsec_...)
	logger        *zap.Logger
}

// NewStripeHandler creates a StripeHandler.
//
//   - apiKey:        STRIPE_SECRET_KEY — used for outbound Stripe API calls.
//   - webhookSecret: STRIPE_WEBHOOK_SECRET — used to validate inbound event signatures.
func NewStripeHandler(apiKey, webhookSecret string, store Store, logger *zap.Logger) *StripeHandler {
	stripe.Key = apiKey
	return &StripeHandler{
		store:         store,
		webhookSecret: webhookSecret,
		logger:        logger,
	}
}

// ServeHTTP is the net/http handler for POST /webhooks/stripe.
// It validates the Stripe-Signature header before dispatching.
func (h *StripeHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	const maxBodyBytes = 65_536 // 64 KiB — Stripe recommends capping to prevent DoS
	r.Body = http.MaxBytesReader(w, r.Body, maxBodyBytes)

	payload, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "request body too large", http.StatusRequestEntityTooLarge)
		return
	}

	event, err := webhook.ConstructEvent(payload, r.Header.Get("Stripe-Signature"), h.webhookSecret)
	if err != nil {
		h.logger.Warn("stripe webhook signature validation failed", zap.Error(err))
		http.Error(w, "invalid signature", http.StatusBadRequest)
		return
	}

	switch event.Type {
	case "invoice.payment_succeeded":
		h.onInvoicePaid(r.Context(), event)
	case "invoice.payment_failed":
		h.onInvoicePaymentFailed(r.Context(), event)
	case "customer.subscription.deleted":
		h.onSubscriptionDeleted(r.Context(), event)
	default:
		// ACK all unhandled types — Stripe retries on any non-2xx response.
		h.logger.Debug("unhandled stripe event", zap.String("type", string(event.Type)))
	}

	// Always return 200 if signature validation passed, even for unhandled events.
	w.WriteHeader(http.StatusOK)
}

// ── event handlers ────────────────────────────────────────────────────────────

func (h *StripeHandler) onInvoicePaid(ctx context.Context, event stripe.Event) {
	var inv stripe.Invoice
	if err := json.Unmarshal(event.Data.Raw, &inv); err != nil {
		h.logger.Error("unmarshal invoice.payment_succeeded failed", zap.Error(err))
		return
	}
	if inv.ID == "" {
		return
	}

	existing, err := h.store.GetInvoiceByStripeID(ctx, inv.ID)
	if err != nil {
		h.logger.Warn("invoice not found for stripe payment_succeeded event",
			zap.String("stripe_invoice_id", inv.ID))
		return
	}

	now := time.Now().UTC()
	if err := h.store.UpdateStatus(ctx, existing.ID, StatusPaid, &now); err != nil {
		h.logger.Error("failed to mark invoice paid",
			zap.String("invoice_id", existing.ID), zap.Error(err))
		return
	}
	h.logger.Info("invoice marked paid",
		zap.String("invoice_id", existing.ID),
		zap.String("stripe_invoice_id", inv.ID))
}

func (h *StripeHandler) onInvoicePaymentFailed(ctx context.Context, event stripe.Event) {
	var inv stripe.Invoice
	if err := json.Unmarshal(event.Data.Raw, &inv); err != nil {
		h.logger.Error("unmarshal invoice.payment_failed failed", zap.Error(err))
		return
	}
	if inv.ID == "" {
		return
	}

	existing, err := h.store.GetInvoiceByStripeID(ctx, inv.ID)
	if err != nil {
		h.logger.Warn("invoice not found for stripe payment_failed event",
			zap.String("stripe_invoice_id", inv.ID))
		return
	}

	// Leave as 'open' — Stripe will retry automatically per the subscription's
	// payment retry schedule.
	if err := h.store.UpdateStatus(ctx, existing.ID, StatusOpen, nil); err != nil {
		h.logger.Error("failed to reset invoice to open on payment failure",
			zap.String("invoice_id", existing.ID), zap.Error(err))
	}
}

func (h *StripeHandler) onSubscriptionDeleted(ctx context.Context, event stripe.Event) {
	var sub stripe.Subscription
	if err := json.Unmarshal(event.Data.Raw, &sub); err != nil {
		h.logger.Error("unmarshal customer.subscription.deleted failed", zap.Error(err))
		return
	}
	customerID := ""
	if sub.Customer != nil {
		customerID = sub.Customer.ID
	}
	// Tier downgrade happens at next provisioning sync.
	// Logged here for the ops runbook / alerting pipeline.
	h.logger.Warn("stripe subscription cancelled — tenant will be downgraded",
		zap.String("stripe_customer_id", customerID),
		zap.String("subscription_id", sub.ID))
}

// ── Stripe customer provisioning ──────────────────────────────────────────────

// CreateCustomer creates a Stripe customer for the given tenant and persists the
// customer ID. Idempotent: if the tenant already has a customer ID it is returned
// unchanged without a second Stripe API call.
func (h *StripeHandler) CreateCustomer(ctx context.Context, tenantID, name, email string) (string, error) {
	existing, err := h.store.GetStripeCustomerID(ctx, tenantID)
	if err != nil {
		return "", fmt.Errorf("billing: get stripe customer id: %w", err)
	}
	if existing != "" {
		return existing, nil
	}

	params := &stripe.CustomerParams{
		Name:  stripe.String(name),
		Email: stripe.String(email),
		Metadata: map[string]string{
			"blazil_tenant_id": tenantID,
		},
	}
	c, err := customer.New(params)
	if err != nil {
		return "", fmt.Errorf("billing: create stripe customer: %w", err)
	}

	if err := h.store.SetStripeCustomerID(ctx, tenantID, c.ID); err != nil {
		return "", fmt.Errorf("billing: save stripe customer id (customer=%s): %w", c.ID, err)
	}
	return c.ID, nil
}

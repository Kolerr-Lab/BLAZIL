// Package billing implements invoice persistence and Stripe payment integration
// for Blazil Cloud's metered billing system.
//
// Lifecycle:
//  1. Admin calls generateInvoice: metering windows → CalculateInvoice → persisted as 'draft'
//  2. Admin calls createStripeCustomer: tenant gets a Stripe customer ID
//  3. Invoice pushed to Stripe (future automation) → status transitions to 'open'
//  4. Stripe fires invoice.payment_succeeded → StripeHandler marks 'paid'
//  5. Stripe fires invoice.payment_failed   → StripeHandler leaves 'open' for retry
package billing

import "time"

// InvoiceStatus models the Stripe-aligned invoice lifecycle.
type InvoiceStatus string

const (
	StatusDraft InvoiceStatus = "draft"
	StatusOpen  InvoiceStatus = "open"
	StatusPaid  InvoiceStatus = "paid"
	StatusVoid  InvoiceStatus = "void"
)

// Invoice is a persisted billing record for one tenant for one calendar period.
type Invoice struct {
	ID              string        `json:"id"`
	TenantID        string        `json:"tenant_id"`
	StripeInvoiceID string        `json:"stripe_invoice_id,omitempty"`
	PeriodStart     time.Time     `json:"period_start"`
	PeriodEnd       time.Time     `json:"period_end"`
	TotalMicroUSD   int64         `json:"total_micro_usd"`
	Status          InvoiceStatus `json:"status"`
	CreatedAt       time.Time     `json:"created_at"`
	PaidAt          *time.Time    `json:"paid_at,omitempty"`
	Lines           []LineItem    `json:"lines,omitempty"`
}

// LineItem is one metering window within an invoice.
// Pricing is captured at generation time — history is immutable.
type LineItem struct {
	ID              string    `json:"id"`
	InvoiceID       string    `json:"invoice_id"`
	WindowStart     time.Time `json:"window_start"`
	TxCount         int64     `json:"tx_count"`
	PricePerTxMicro int64     `json:"price_per_tx_micro"`
	TotalMicroUSD   int64     `json:"total_micro_usd"`
}

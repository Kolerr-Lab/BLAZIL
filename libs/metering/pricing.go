package metering

import "time"

// Tier identifies a billing tier.
type Tier string

const (
	// TierFree is the open-source self-hosted tier (no per-tx billing).
	TierFree Tier = "free"
	// TierCloudSaaS is the managed Cloud SaaS tier with per-tx volume billing.
	TierCloudSaaS Tier = "cloud_saas"
	// TierEnterprise is the annual contract tier ($250-500K/yr flat fee).
	TierEnterprise Tier = "enterprise"
)

// PricePerTxMicroUSD returns the per-transaction price in micro-USD
// (1 micro-USD = $0.000001) for a Cloud SaaS tenant whose cumulative
// transaction count for the current billing month is cumulativeMonthly.
//
// Volume tier schedule:
//
//	       0 – 999,999   tx/mo  →  $0.001000  (1_000 µ$)
//	  1,000,000 – 9,999,999      →  $0.000500  (  500 µ$)
//	 10,000,000 – 99,999,999     →  $0.000200  (  200 µ$)
//	100,000,000+                 →  $0.000100  (  100 µ$)  (enterprise-annual terms)
//
// Returns 0 for TierFree and TierEnterprise (billing handled externally).
func PricePerTxMicroUSD(tier Tier, cumulativeMonthly int64) int64 {
	if tier != TierCloudSaaS {
		return 0
	}
	switch {
	case cumulativeMonthly < 1_000_000:
		return 1_000 // $0.001
	case cumulativeMonthly < 10_000_000:
		return 500 // $0.0005
	case cumulativeMonthly < 100_000_000:
		return 200 // $0.0002
	default:
		return 100 // $0.0001
	}
}

// WindowCount holds the transaction count for a single metering window.
type WindowCount struct {
	// WindowStart is the UTC start of the 60-second window.
	WindowStart time.Time
	// Count is the number of confirmed transactions in the window.
	Count int64
}

// InvoiceLineItem represents one line on a billing invoice.
type InvoiceLineItem struct {
	TenantID      string
	WindowStart   time.Time
	TxCount       int64
	PricePerTxµ   int64 // price per transaction in micro-USD
	TotalMicroUSD int64 // TxCount × PricePerTxµ
}

// CalculateInvoice computes invoice line items for a tenant's windowed usage.
//
// counts must be ordered by WindowStart ascending so that the cumulative volume
// accumulates correctly for tier transitions.
//
// This function is pure (no I/O) and may be called from any goroutine.
func CalculateInvoice(tenantID string, tier Tier, counts []WindowCount) []InvoiceLineItem {
	lines := make([]InvoiceLineItem, 0, len(counts))
	var cumulative int64
	for _, wc := range counts {
		pricePerTx := PricePerTxMicroUSD(tier, cumulative)
		lines = append(lines, InvoiceLineItem{
			TenantID:      tenantID,
			WindowStart:   wc.WindowStart,
			TxCount:       wc.Count,
			PricePerTxµ:   pricePerTx,
			TotalMicroUSD: pricePerTx * wc.Count,
		})
		cumulative += wc.Count
	}
	return lines
}

// TotalInvoiceMicroUSD sums all line item totals.
func TotalInvoiceMicroUSD(lines []InvoiceLineItem) int64 {
	var total int64
	for _, l := range lines {
		total += l.TotalMicroUSD
	}
	return total
}

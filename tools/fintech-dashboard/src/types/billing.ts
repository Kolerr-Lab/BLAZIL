// Shared types for billing domain — mirrors the Go admin API JSON shapes.

export type TenantTier = "free" | "cloud_saas" | "enterprise";

export interface Tenant {
  id: string;
  name: string;
  email: string;
  tier: TenantTier;
  rate_limit_rps: number;
  rate_limit_burst: number;
  created_at: string;
  suspended_at: string | null;
}

export type InvoiceStatus = "draft" | "open" | "paid" | "void";

export interface LineItem {
  id: string;
  invoice_id: string;
  window_start: string;
  tx_count: number;
  price_per_tx_micro: number;
  total_micro_usd: number;
}

export interface Invoice {
  id: string;
  tenant_id: string;
  stripe_invoice_id?: string;
  period_start: string;
  period_end: string;
  total_micro_usd: number;
  status: InvoiceStatus;
  created_at: string;
  paid_at?: string;
  lines?: LineItem[];
}

export interface UsageWindow {
  tenant_id: string;
  window_start: string;
  window_end: string;
  tx_count: number;
}

export interface UsageResponse {
  tenant_id: string;
  windows: UsageWindow[];
}

export interface InvoicePreview {
  tenant_id: string;
  tier: TenantTier;
  lines: {
    tenant_id: string;
    window_start: string;
    tx_count: number;
    price_per_tx_micro: number;
    total_micro_usd: number;
  }[];
  total_micro_usd: number;
  period: { year: number; month: number };
}

// Convert micro-USD (integer) to a human-readable dollar string.
// 1_000_000 µ$ = $1.00
export function microUSDtoDisplay(microUSD: number): string {
  if (microUSD === 0) return "$0.00";
  const dollars = microUSD / 1_000_000;
  return `$${dollars.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`;
}

export const TIER_LABELS: Record<TenantTier, string> = {
  free: "Free",
  cloud_saas: "Cloud SaaS",
  enterprise: "Enterprise",
};

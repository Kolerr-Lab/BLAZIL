import Link from "next/link";
import { notFound } from "next/navigation";
import type { Tenant, UsageResponse, InvoicePreview, Invoice } from "@/types/billing";
import { microUSDtoDisplay } from "@/types/billing";
import { TierBadge } from "@/components/billing/TierBadge";
import { UsageBarChart } from "@/components/billing/UsageBarChart";
import { InvoiceTable } from "@/components/billing/InvoiceTable";

const BASE = process.env.NEXT_PUBLIC_APP_URL ?? "http://localhost:3330";
const NO_CACHE = { cache: "no-store" } as const;

async function fetchTenant(id: string): Promise<Tenant | null> {
  // Reuse the admin proxy route for single tenant
  const res = await fetch(`${BASE}/api/tenants`, NO_CACHE);
  if (!res.ok) return null;
  const all: Tenant[] = await res.json();
  return all.find((t) => t.id === id) ?? null;
}

async function fetchUsage(id: string): Promise<UsageResponse | null> {
  const res = await fetch(`${BASE}/api/tenants/${id}/usage`, NO_CACHE);
  if (!res.ok) return null;
  return res.json();
}

async function fetchInvoicePreview(id: string): Promise<InvoicePreview | null> {
  const res = await fetch(`${BASE}/api/tenants/${id}/invoice`, NO_CACHE);
  if (!res.ok) return null;
  return res.json();
}

async function fetchInvoices(id: string): Promise<Invoice[]> {
  const res = await fetch(`${BASE}/api/tenants/${id}/invoices`, NO_CACHE);
  if (!res.ok) return [];
  const data = await res.json();
  return Array.isArray(data) ? data : [];
}

type Props = { params: Promise<{ id: string }> };

export default async function TenantBillingPage({ params }: Props) {
  const { id } = await params;
  const [tenant, usage, preview, invoices] = await Promise.all([
    fetchTenant(id),
    fetchUsage(id),
    fetchInvoicePreview(id),
    fetchInvoices(id),
  ]);

  if (!tenant) notFound();

  const totalTx = usage?.windows?.reduce((s, w) => s + w.tx_count, 0) ?? 0;

  return (
    <div className="min-h-screen p-6" style={{ background: "var(--bg)", color: "var(--fg)" }}>
      <div className="max-w-5xl mx-auto space-y-6">

        {/* ── Breadcrumb ───────────────────────────────────────────────────── */}
        <div className="flex items-center gap-2 text-sm text-gray-400">
          <Link href="/" className="hover:text-blue-400 transition-colors">Dashboard</Link>
          <span>/</span>
          <Link href="/billing/tenants" className="hover:text-blue-400 transition-colors">Tenants</Link>
          <span>/</span>
          <span className="text-white">{tenant.name}</span>
        </div>

        {/* ── Header ───────────────────────────────────────────────────────── */}
        <div className="rounded-lg border border-gray-700 bg-gray-900 p-5 flex flex-wrap items-start justify-between gap-4">
          <div>
            <div className="flex items-center gap-3 mb-1">
              <h1 className="text-xl font-bold text-white">{tenant.name}</h1>
              <TierBadge tier={tenant.tier} />
              {tenant.suspended_at && (
                <span className="text-xs font-semibold uppercase text-red-400 bg-red-900/30 px-2 py-0.5 rounded">
                  Suspended
                </span>
              )}
            </div>
            <p className="text-sm text-gray-400">{tenant.email}</p>
            <p className="text-xs text-gray-500 mt-1 font-mono">{tenant.id}</p>
          </div>
          <div className="flex gap-6 text-center">
            <div>
              <div className="text-2xl font-bold text-blue-400 font-mono">
                {totalTx.toLocaleString()}
              </div>
              <div className="text-xs text-gray-400 mt-0.5">TX this month</div>
            </div>
            <div>
              <div className="text-2xl font-bold text-green-400 font-mono">
                {preview ? microUSDtoDisplay(preview.total_micro_usd) : "—"}
              </div>
              <div className="text-xs text-gray-400 mt-0.5">Invoice preview</div>
            </div>
          </div>
        </div>

        {/* ── Usage chart ──────────────────────────────────────────────────── */}
        <div className="rounded-lg border border-gray-700 bg-gray-900 p-5">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider mb-4">
            Usage — Current Month
          </h2>
          <UsageBarChart windows={usage?.windows ?? []} />
        </div>

        {/* ── Invoice preview ───────────────────────────────────────────────── */}
        {preview && (
          <div className="rounded-lg border border-gray-700 bg-gray-900 p-5">
            <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider mb-4">
              Invoice Preview — {new Date(0, preview.period.month - 1).toLocaleString("en-US", { month: "long" })} {preview.period.year}
            </h2>
            {preview.lines?.length > 0 ? (
              <>
                <table className="w-full text-sm mb-3">
                  <thead>
                    <tr className="text-gray-400 text-left border-b border-gray-700">
                      <th className="pb-2 pr-4 font-medium">Window</th>
                      <th className="pb-2 pr-4 font-medium text-right">Transactions</th>
                      <th className="pb-2 pr-4 font-medium text-right">Unit price</th>
                      <th className="pb-2 font-medium text-right">Subtotal</th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.lines.map((line, i) => (
                      <tr key={i} className="border-b border-gray-800">
                        <td className="py-1.5 pr-4 text-gray-300 text-xs font-mono">
                          {new Date(line.window_start).toLocaleString()}
                        </td>
                        <td className="py-1.5 pr-4 text-right font-mono text-gray-200">
                          {line.tx_count.toLocaleString()}
                        </td>
                        <td className="py-1.5 pr-4 text-right font-mono text-gray-400 text-xs">
                          {microUSDtoDisplay(line.price_per_tx_micro)}/tx
                        </td>
                        <td className="py-1.5 text-right font-mono text-blue-300">
                          {microUSDtoDisplay(line.total_micro_usd)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <div className="flex justify-end pt-1 border-t border-gray-700">
                  <span className="text-sm font-semibold text-white">
                    Total: {microUSDtoDisplay(preview.total_micro_usd)}
                  </span>
                </div>
              </>
            ) : (
              <p className="text-sm text-gray-400 text-center py-2">
                No billable usage this period.
              </p>
            )}
          </div>
        )}

        {/* ── Persisted invoices ────────────────────────────────────────────── */}
        <div className="rounded-lg border border-gray-700 bg-gray-900 p-5">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider mb-4">
            Invoice History
          </h2>
          <InvoiceTable invoices={invoices} />
        </div>

      </div>
    </div>
  );
}

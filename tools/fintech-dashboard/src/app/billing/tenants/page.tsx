import Link from "next/link";
import type { Tenant } from "@/types/billing";
import { TierBadge } from "@/components/billing/TierBadge";

async function fetchTenants(): Promise<Tenant[]> {
  const base = process.env.NEXT_PUBLIC_APP_URL ?? "http://localhost:3330";
  const res = await fetch(`${base}/api/tenants`, { cache: "no-store" });
  if (!res.ok) return [];
  const data = await res.json();
  return Array.isArray(data) ? data : [];
}

export default async function TenantsPage() {
  const tenants = await fetchTenants();

  return (
    <div className="min-h-screen p-6" style={{ background: "var(--bg)", color: "var(--fg)" }}>
      <div className="max-w-5xl mx-auto">
        <div className="flex items-center justify-between mb-6">
          <div>
            <h1 className="text-2xl font-bold text-white">Tenants</h1>
            <p className="text-sm text-gray-400 mt-0.5">{tenants.length} registered</p>
          </div>
          <Link
            href="/"
            className="text-sm text-blue-400 hover:text-blue-300 transition-colors"
          >
            ← Benchmark dashboard
          </Link>
        </div>

        {tenants.length === 0 ? (
          <div className="rounded-lg border border-gray-700 p-8 text-center text-gray-400">
            No tenants found. Check that ADMIN_API_URL and ADMIN_TOKEN are configured.
          </div>
        ) : (
          <div className="rounded-lg border border-gray-700 overflow-hidden">
            <table className="w-full text-sm">
              <thead className="bg-gray-800">
                <tr className="text-gray-400 text-left">
                  <th className="px-4 py-3 font-medium">Name</th>
                  <th className="px-4 py-3 font-medium">Email</th>
                  <th className="px-4 py-3 font-medium">Tier</th>
                  <th className="px-4 py-3 font-medium">Rate limit</th>
                  <th className="px-4 py-3 font-medium">Status</th>
                  <th className="px-4 py-3 font-medium"></th>
                </tr>
              </thead>
              <tbody>
                {tenants.map((t) => (
                  <tr
                    key={t.id}
                    className="border-t border-gray-700 hover:bg-gray-800/40 transition-colors"
                  >
                    <td className="px-4 py-3 font-medium text-white">{t.name}</td>
                    <td className="px-4 py-3 text-gray-300">{t.email}</td>
                    <td className="px-4 py-3">
                      <TierBadge tier={t.tier} />
                    </td>
                    <td className="px-4 py-3 font-mono text-gray-300">
                      {t.rate_limit_rps} RPS / {t.rate_limit_burst} burst
                    </td>
                    <td className="px-4 py-3">
                      {t.suspended_at ? (
                        <span className="text-xs font-semibold uppercase text-red-400">
                          Suspended
                        </span>
                      ) : (
                        <span className="text-xs font-semibold uppercase text-green-400">
                          Active
                        </span>
                      )}
                    </td>
                    <td className="px-4 py-3 text-right">
                      <Link
                        href={`/billing/tenants/${t.id}`}
                        className="text-xs text-blue-400 hover:text-blue-300 font-medium transition-colors"
                      >
                        View billing →
                      </Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

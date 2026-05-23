import Link from "next/link";

export default function BillingIndexPage() {
  return (
    <div className="min-h-screen p-6" style={{ background: "var(--bg)", color: "var(--fg)" }}>
      <div className="max-w-5xl mx-auto">
        <h1 className="text-2xl font-bold text-white mb-2">Billing</h1>
        <p className="text-gray-400 mb-8">
          Manage tenants, usage metering, and Stripe invoicing.
        </p>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
          <Link
            href="/billing/tenants"
            className="rounded-lg border border-gray-700 bg-gray-900 p-5 hover:border-blue-500 transition-colors group"
          >
            <h2 className="text-lg font-semibold text-white group-hover:text-blue-400 transition-colors">
              Tenants →
            </h2>
            <p className="text-sm text-gray-400 mt-1">
              View all tenants, their tiers, usage this month, and invoice history.
            </p>
          </Link>
          <Link
            href="/"
            className="rounded-lg border border-gray-700 bg-gray-900 p-5 hover:border-blue-500 transition-colors group"
          >
            <h2 className="text-lg font-semibold text-white group-hover:text-blue-400 transition-colors">
              Benchmark Dashboard →
            </h2>
            <p className="text-sm text-gray-400 mt-1">
              Real-time TPS, latency, and cluster health metrics.
            </p>
          </Link>
        </div>
      </div>
    </div>
  );
}

import type { Invoice } from "@/types/billing";
import { microUSDtoDisplay } from "@/types/billing";
import clsx from "clsx";

const STATUS_CLASSES: Record<string, string> = {
  draft: "bg-gray-600 text-gray-200",
  open: "bg-yellow-700 text-yellow-100",
  paid: "bg-green-700 text-green-100",
  void: "bg-red-800 text-red-200",
};

export function InvoiceTable({ invoices }: { invoices: Invoice[] }) {
  if (!invoices?.length) {
    return (
      <p className="text-sm text-gray-400 py-4 text-center">
        No invoices yet for this tenant.
      </p>
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-sm border-collapse">
        <thead>
          <tr className="text-gray-400 text-left border-b border-gray-700">
            <th className="pb-2 pr-4 font-medium">Period</th>
            <th className="pb-2 pr-4 font-medium">Total</th>
            <th className="pb-2 pr-4 font-medium">Status</th>
            <th className="pb-2 font-medium">Created</th>
          </tr>
        </thead>
        <tbody>
          {invoices.map((inv) => (
            <tr key={inv.id} className="border-b border-gray-800 hover:bg-gray-800/40 transition-colors">
              <td className="py-2 pr-4 text-gray-200">
                {new Date(inv.period_start).toLocaleDateString("en-US", {
                  month: "short",
                  year: "numeric",
                })}
              </td>
              <td className="py-2 pr-4 font-mono text-blue-300">
                {microUSDtoDisplay(inv.total_micro_usd)}
              </td>
              <td className="py-2 pr-4">
                <span
                  className={clsx(
                    "rounded px-2 py-0.5 text-xs font-semibold uppercase",
                    STATUS_CLASSES[inv.status] ?? "bg-gray-600 text-gray-200"
                  )}
                >
                  {inv.status}
                </span>
              </td>
              <td className="py-2 text-gray-400 text-xs">
                {new Date(inv.created_at).toLocaleDateString()}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

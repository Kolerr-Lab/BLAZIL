import clsx from "clsx";
import type { TenantTier } from "@/types/billing";
import { TIER_LABELS } from "@/types/billing";

const TIER_CLASSES: Record<TenantTier, string> = {
  free: "bg-gray-700 text-gray-200",
  cloud_saas: "bg-blue-700 text-blue-100",
  enterprise: "bg-purple-700 text-purple-100",
};

export function TierBadge({ tier }: { tier: TenantTier }) {
  return (
    <span
      className={clsx(
        "inline-block rounded px-2 py-0.5 text-xs font-semibold uppercase tracking-wide",
        TIER_CLASSES[tier] ?? "bg-gray-600 text-gray-100"
      )}
    >
      {TIER_LABELS[tier] ?? tier}
    </span>
  );
}

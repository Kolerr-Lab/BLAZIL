"use client";

// Static DigitalOcean droplet spec for the VSR bench cluster.
// Update NODE_COUNT or DROPLET if the infra changes.

const NODE_COUNT = 3;

const DROPLET = {
  cpu: "4 AMD vCPUs",
  ram: "8 GB",
  disk: "160 GB NVMe SSD",
  transfer: "5 TB",
  price: "$56/mo",
} as const;

const SPECS = [
  { icon: "⬡", label: "Nodes", value: `${NODE_COUNT}× DO Droplet` },
  { icon: "▣", label: "CPU", value: DROPLET.cpu },
  { icon: "◈", label: "RAM", value: DROPLET.ram },
  { icon: "◉", label: "Disk", value: DROPLET.disk },
  { icon: "⇅", label: "Transfer", value: DROPLET.transfer },
  { icon: "$", label: "Cost", value: `${DROPLET.price} × ${NODE_COUNT} = $${56 * NODE_COUNT}/mo` },
] as const;

export function ClusterInfo() {
  return (
    <div
      className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3 py-2 rounded-lg text-[10px]"
      style={{
        background: "color-mix(in srgb, var(--accent-blue) 5%, var(--bg-card))",
        border: "1px solid color-mix(in srgb, var(--accent-blue) 14%, transparent)",
      }}
    >
      <span
        className="font-semibold uppercase tracking-widest shrink-0"
        style={{ color: "var(--accent-blue)", fontSize: "9px" }}
      >
        Test Environment
      </span>

      <div
        className="w-px h-3 shrink-0"
        style={{ background: "var(--border-bright)" }}
      />

      {SPECS.map(({ label, value }) => (
        <div key={label} className="flex items-center gap-1.5 shrink-0">
          <span style={{ color: "var(--text-dim)" }}>{label}</span>
          <span className="font-semibold tabular-nums" style={{ color: "var(--text)" }}>
            {value}
          </span>
        </div>
      ))}
    </div>
  );
}

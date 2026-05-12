"use client";

// AWS i4i.4xlarge single-node VSR bench — 3 TB replicas on 1 instance.
// Update when infra changes.

const NODE_COUNT = 1;

const INSTANCE = {
  type: "i4i.4xlarge",
  cpu: "16 vCPU (Intel Ice Lake)",
  ram: "128 GB",
  disk: "1× 1.9 TB NVMe (instance store)",
  network: "Up to 25 Gbps",
  price: "$1.248/hr",
} as const;

const SPECS = [
  { label: "Instance", value: `${NODE_COUNT}× AWS ${INSTANCE.type}` },
  { label: "CPU", value: INSTANCE.cpu },
  { label: "RAM", value: INSTANCE.ram },
  { label: "Disk", value: INSTANCE.disk },
  { label: "Network", value: INSTANCE.network },
  { label: "Cost", value: INSTANCE.price },
  { label: "TB Nodes", value: "3 replicas (VSR 2-of-3)" },
  { label: "Window", value: "1 024 / shard" },
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

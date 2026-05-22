"use client";

// AWS i4i.16xlarge single-node VSR bench — 3 TB replicas on 1 instance.
// Update when infra changes.

const NODE_COUNT = 1;

const INSTANCE = {
  type: "i4i.16xlarge",
  cpu: "64 vCPU (Intel Ice Lake)",
  ram: "512 GiB",
  disk: "4× 3.75 TB NVMe (instance store)",
  network: "Up to 75 Gbps",
  price: "$5.014/hr",
} as const;

interface Props {
  windowPerShard?: number;
}

export function ClusterInfo({ windowPerShard }: Props) {
  const specs = [
    { label: "Instance", value: `${NODE_COUNT}× AWS ${INSTANCE.type}` },
    { label: "CPU", value: INSTANCE.cpu },
    { label: "RAM", value: INSTANCE.ram },
    { label: "Disk", value: INSTANCE.disk },
    { label: "Network", value: INSTANCE.network },
    { label: "Cost", value: INSTANCE.price },
    { label: "TB Nodes", value: "3 replicas (VSR 2-of-3)" },
    {
      label: "Window",
      value: windowPerShard != null
        ? `${windowPerShard.toLocaleString()} / shard`
        : "32 768 / shard",
    },
  ];

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

      {specs.map(({ label, value }) => (
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

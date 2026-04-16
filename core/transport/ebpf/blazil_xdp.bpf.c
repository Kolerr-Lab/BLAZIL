// SPDX-License-Identifier: GPL-2.0
//
// blazil_xdp.bpf.c — XDP Gatekeeper for Blazil AF_XDP ingress
//
// Attaches at XDP_DRV (native mode) on the data-plane NIC.
//
// Packet classification logic:
//   1. Ethernet / IPv4 / UDP check — all other L3/L4 protocols → XDP_PASS
//      (keeps SSH, ARP, ICMP, management traffic flowing through the kernel).
//   2. UDP destination port must equal BLAZIL_UDP_PORT (7878).
//   3. First 4 bytes of UDP payload must be the Blazil magic  0x424C5A4C ("BLZL").
//   If (2) and (3) both match  → bpf_redirect_map(&xsks_map, rx_queue_index, XDP_PASS)
//      XDP_PASS fallback: if no AF_XDP socket is registered for this queue
//      (e.g. during startup), the packet falls back to the kernel normally.
//   Otherwise → XDP_PASS  (let the kernel handle it; never DROP management traffic)
//
// Zero-copy path:
//   The BPF program redirects the packet's DMA buffer directly into the
//   AF_XDP socket's RX ring via the XSKMAP without any copy.
//
// Wire frame layout (inspected here):
//   [ Ethernet 14B ][ IPv4 20B* ][ UDP 8B ][ MAGIC 4B ][ MsgPack payload N B ]
//   * minimum IHL=5; we handle IHL options via ihl*4.
//
// Compile (called from build.rs):
//   clang -target bpf -O2 -g -Wall \
//     -I/usr/include/$(uname -m)-linux-gnu \
//     -c ebpf/blazil_xdp.bpf.c -o $OUT_DIR/blazil_xdp.bpf.o
//
// Note: Requires kernel 4.18+ and a driver with XDP_DRV support
// (ixgbe, i40e, mlx5_core, ena ≥ 5.10 with XDP, virtio_net ≥ 5.10).

#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/udp.h>
#include <linux/in.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_endian.h>

// ── Constants ─────────────────────────────────────────────────────────────────

/// UDP destination port that Blazil AF_XDP clients connect to.
/// Must match AfXdpConfig::port in the Rust code.
#define BLAZIL_UDP_PORT  7878

/// 4-byte magic header at the start of every Blazil UDP payload.
/// ASCII "BLZL" = 0x42 0x4C 0x5A 0x4C
#define BLAZIL_MAGIC_BE  0x424C5A4CLU

/// Maximum number of NIC queues / AF_XDP sockets we support.
/// Increase if you run more than 64 RX queues (rare even on i4i.metal).
#define XSKMAP_MAX_ENTRIES 64

// ── BPF Maps ──────────────────────────────────────────────────────────────────

/// XSKMAP: maps RX queue index → AF_XDP socket file descriptor.
/// Populated by the Rust userspace loader (ebpf/mod.rs) when each
/// XSocket is bound. Key = queue_id (u32), value = socket fd (int).
struct {
    __uint(type,       BPF_MAP_TYPE_XSKMAP);
    __uint(max_entries, XSKMAP_MAX_ENTRIES);
    __type(key,        __u32);
    __type(value,      __u32);
} xsks_map SEC(".maps");

// ── Helper: safe bounds-checked pointer advance ───────────────────────────────

static __always_inline int bounds_ok(void *ptr, __u32 size, void *data_end)
{
    return (void *)((char *)ptr + size) <= data_end;
}

// ── XDP Entry Point ───────────────────────────────────────────────────────────

SEC("xdp")
int blazil_xdp_filter(struct xdp_md *ctx)
{
    void *data     = (void *)(long)ctx->data;
    void *data_end = (void *)(long)ctx->data_end;

    // ── Layer 2: Ethernet ─────────────────────────────────────────────────────
    struct ethhdr *eth = data;
    if (!bounds_ok(eth, sizeof(*eth), data_end))
        return XDP_PASS;

    // Only handle IPv4; let IPv6, ARP, VLAN etc. pass to kernel.
    if (bpf_ntohs(eth->h_proto) != ETH_P_IP)
        return XDP_PASS;

    // ── Layer 3: IPv4 ─────────────────────────────────────────────────────────
    struct iphdr *ip = (struct iphdr *)(eth + 1);
    if (!bounds_ok(ip, sizeof(*ip), data_end))
        return XDP_PASS;

    // Only handle UDP; leave TCP (SSH/TB VSR), ICMP etc. to kernel.
    if (ip->protocol != IPPROTO_UDP)
        return XDP_PASS;

    // Compute the real IP header length (handles options, IHL > 5).
    // IHL field is in 32-bit words; multiply by 4 for bytes.
    // Clamp to 60 (max IHL) to satisfy the BPF verifier's loop bounds.
    __u32 ip_hlen = (__u32)(ip->ihl) * 4;
    if (ip_hlen < 20)
        return XDP_PASS;  // malformed

    // ── Layer 4: UDP ──────────────────────────────────────────────────────────
    struct udphdr *udp = (struct udphdr *)((char *)ip + ip_hlen);
    if (!bounds_ok(udp, sizeof(*udp), data_end))
        return XDP_PASS;

    // Port check — not a Blazil packet → pass.
    if (bpf_ntohs(udp->dest) != BLAZIL_UDP_PORT)
        return XDP_PASS;

    // ── Application: Blazil magic ─────────────────────────────────────────────
    // First 4 bytes of UDP payload must be the Blazil magic word.
    __u32 *magic_ptr = (__u32 *)((char *)udp + sizeof(*udp));
    if (!bounds_ok(magic_ptr, sizeof(__u32), data_end))
        return XDP_PASS;  // too short to contain magic

    // Compare in network byte order (big-endian).
    if (*magic_ptr != bpf_htonl(BLAZIL_MAGIC_BE))
        return XDP_PASS;  // wrong magic → not a Blazil frame

    // ── Redirect to AF_XDP socket ─────────────────────────────────────────────
    // bpf_redirect_map with XDP_PASS as fallback:
    //   - If an AF_XDP socket is registered for this RX queue → redirect (zero-copy)
    //   - If no socket is registered yet (startup race) → fallback XDP_PASS
    // The UMEM DMA buffer is transferred directly to the AF_XDP RX ring;
    // no copy of the packet bytes occurs anywhere in this path.
    return bpf_redirect_map(&xsks_map, ctx->rx_queue_index, XDP_PASS);
}

char _license[] SEC("license") = "GPL";

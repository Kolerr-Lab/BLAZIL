provider "digitalocean" {
  token = var.do_token
}

# ── SSH Key ───────────────────────────────────────────────────────────────────

resource "digitalocean_ssh_key" "blazil" {
  name       = "blazil-cluster"
  public_key = file(var.ssh_public_key_path)
}

# ── VPC (private networking for TB VSR + engine mesh) ────────────────────────

resource "digitalocean_vpc" "blazil" {
  name     = "blazil-vpc"
  region   = var.region
  ip_range = var.vpc_cidr
}

# ── Droplets ──────────────────────────────────────────────────────────────────
#
# 3 nodes, each running:
#   - TigerBeetle replica (one of 3 VSR replicas)
#   - blazil-engine        (one shard owner)
#   - payments / banking / trading / crypto (Go gRPC services)
#   - Prometheus (node-1 only: + Grafana)

resource "digitalocean_droplet" "node" {
  count  = var.node_count
  name   = "blazil-node-${count.index + 1}"
  image  = var.image
  size   = var.droplet_size
  region = var.region

  vpc_uuid         = digitalocean_vpc.blazil.id
  ssh_keys         = [digitalocean_ssh_key.blazil.fingerprint]
  graceful_shutdown = true  # wait for TB to checkpoint before power-off

  tags = [
    "blazil",
    "blazil-node-${count.index + 1}",
    "shard-${count.index}",
    var.project_name,
  ]
}

# ── Persistent volumes for TigerBeetle data ───────────────────────────────────
#
# TigerBeetle pre-allocates its data file on first start. Once created, never
# shrink the volume — TB will refuse to start if it finds a truncated data file.

resource "digitalocean_volume" "tb_data" {
  count  = var.node_count
  name   = "blazil-tb-data-${count.index}"
  region = var.region
  size   = var.tb_data_volume_size_gb

  initial_filesystem_type  = "ext4"
  initial_filesystem_label = "blazil-tb-${count.index}"

  tags = ["blazil", "tigerbeetle"]
}

resource "digitalocean_volume_attachment" "tb_data" {
  count      = var.node_count
  droplet_id = digitalocean_droplet.node[count.index].id
  volume_id  = digitalocean_volume.tb_data[count.index].id
}

# ── Firewall ──────────────────────────────────────────────────────────────────

resource "digitalocean_firewall" "blazil" {
  name        = "blazil-cluster"
  droplet_ids = digitalocean_droplet.node[*].id

  # SSH — restrict to management CIDR in production
  inbound_rule {
    protocol         = "tcp"
    port_range       = "22"
    source_addresses = [var.management_cidr]
  }

  # TigerBeetle VSR inter-replica — VPC only
  inbound_rule {
    protocol         = "tcp"
    port_range       = "3000-3002"
    source_addresses = [var.vpc_cidr]
  }

  # Blazil engine (Aeron + TCP transport) — VPC only (Go services talk to engine)
  inbound_rule {
    protocol         = "tcp"
    port_range       = "7878-7880"
    source_addresses = [var.vpc_cidr]
  }

  # gRPC services — public ingress (payments, banking, trading, crypto)
  # node-1: 50051-50054, node-2: 50061-50064, node-3: 50071-50074
  inbound_rule {
    protocol         = "tcp"
    port_range       = "50051-50074"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }

  # Prometheus — VPC only (scraped by node-1 prometheus)
  inbound_rule {
    protocol         = "tcp"
    port_range       = "9090-9097"
    source_addresses = [var.vpc_cidr]
  }

  # Grafana — public (restrict to VPN/office IP in production)
  inbound_rule {
    protocol         = "tcp"
    port_range       = "3001"
    source_addresses = ["0.0.0.0/0", "::/0"]
  }

  # Aeron cross-node UDP (when using aeron:udp across nodes)
  inbound_rule {
    protocol         = "udp"
    port_range       = "20121"
    source_addresses = [var.vpc_cidr]
  }

  # Allow all outbound
  outbound_rule {
    protocol              = "tcp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }

  outbound_rule {
    protocol              = "udp"
    port_range            = "1-65535"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }

  outbound_rule {
    protocol              = "icmp"
    destination_addresses = ["0.0.0.0/0", "::/0"]
  }
}

# ── DO Project (console grouping) ─────────────────────────────────────────────

resource "digitalocean_project" "blazil" {
  name        = var.project_name
  description = "Blazil high-throughput payment engine — 1M TPS Aeron IPC, TigerBeetle VSR"
  purpose     = "Service or API"
  environment = "Production"

  resources = concat(
    digitalocean_droplet.node[*].urn,
    digitalocean_volume.tb_data[*].urn,
  )
}

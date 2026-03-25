# Terraform — DigitalOcean

Provisions a 3-node Blazil cluster on DigitalOcean:
- 3× droplets (`s-8vcpu-16gb` default, or `c-8` for CPU-optimized benchmark runs)
- Private VPC for internal traffic (TigerBeetle VSR, engine mesh)
- Firewall rules (gRPC public, TB/engine VPC-only, SSH restricted)
- Persistent block volumes for TigerBeetle data (50GB each)
- DigitalOcean project for console grouping

## Prerequisites

```bash
brew install terraform
# or: https://developer.hashicorp.com/terraform/downloads
terraform version  # >= 1.5
```

## Quick Start

```bash
cd infra/terraform/digitalocean

# Set your DO API token (never commit this)
export TF_VAR_do_token="dop_v1_..."

# Optional: use CPU-optimized droplets for benchmarks (~$534/month total)
# export TF_VAR_droplet_size="c-8"

terraform init
terraform plan
terraform apply
```

## After Apply

Terraform outputs everything needed to start the cluster:

```bash
# Get the start commands for each node
terraform output -json do_start_commands

# Get TB_ADDRESSES and BLAZIL_NODES for manual use
terraform output tb_addresses
terraform output blazil_nodes

# Open Grafana
terraform output grafana_url
```

Then on each node:
```bash
# 1. Run setup (once per node)
ssh root@<node-ip> 'bash -s' < scripts/do-setup.sh -- node-1 0  # adjust per node

# 2. Run kernel tuning (once per node)
ssh root@<node-ip> 'bash -s' < scripts/do-tune.sh

# 3. Start the node
ssh root@<node-ip> "cd /opt/blazil && \
  BLAZIL_NODE_ID=node-1 BLAZIL_SHARD_ID=0 \
  ./scripts/do-start.sh <node1-private-ip> <node2-private-ip> <node3-private-ip>"
```

Or use the Ansible playbook for one-command cluster bring-up:
```bash
cd infra/ansible
ansible-playbook -i inventory/production playbooks/site.yml \
  -e "tb_addresses=$(cd ../terraform/digitalocean && terraform output -raw tb_addresses)" \
  -e "blazil_nodes=$(cd ../terraform/digitalocean && terraform output -raw blazil_nodes)"
```

## Variables

| Variable | Default | Description |
|---|---|---|
| `do_token` | — | DigitalOcean API token (required, sensitive) |
| `region` | `nyc3` | DO region slug |
| `droplet_size` | `s-8vcpu-16gb` | Droplet size (use `c-8` for benchmark) |
| `node_count` | `3` | Must be 3 (TigerBeetle VSR quorum) |
| `ssh_public_key_path` | `~/.ssh/id_ed25519.pub` | SSH key for node access |
| `vpc_cidr` | `10.10.0.0/24` | Private VPC range |
| `tb_data_volume_size_gb` | `50` | TigerBeetle data volume (never shrink) |
| `management_cidr` | `0.0.0.0/0` | Restrict to your IP in production |

## Cost

| Config | Monthly |
|---|---|
| 3× `s-8vcpu-16gb` (general) | ~$252 |
| 3× `c-8` (CPU-optimized) | ~$534 |
| 3× volumes (50GB each) | ~$15 |

## Teardown

```bash
terraform destroy
```

> **Warning:** This deletes the TigerBeetle data volumes. All ledger data will be
> permanently lost. Back up via `scripts/backup-restore.sh` before destroying.

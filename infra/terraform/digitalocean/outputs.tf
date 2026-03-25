output "node_public_ips" {
  description = "Public IPs of all Blazil nodes (use for SSH and external access)"
  value       = digitalocean_droplet.node[*].ipv4_address
}

output "node_private_ips" {
  description = "Private VPC IPs (use for TB_ADDRESSES and BLAZIL_NODES — never expose publicly)"
  value       = digitalocean_droplet.node[*].ipv4_address_private
  sensitive   = false
}

output "tb_addresses" {
  description = "TigerBeetle VSR cluster addresses string — pass as TB_ADDRESSES env var"
  value = join(",", [
    for i, ip in digitalocean_droplet.node[*].ipv4_address_private :
    "${ip}:${3000 + i}"
  ])
}

output "blazil_nodes" {
  description = "Blazil inter-engine mesh — pass as BLAZIL_NODES env var"
  value = join(",", [
    for i, ip in digitalocean_droplet.node[*].ipv4_address_private :
    "node-${i + 1}:${ip}:7878"
  ])
}

output "grafana_url" {
  description = "Grafana dashboard URL (hosted on node-1)"
  value       = "http://${digitalocean_droplet.node[0].ipv4_address}:3001"
}

output "ssh_commands" {
  description = "SSH commands for each node"
  value = {
    for i, node in digitalocean_droplet.node :
    "node-${i + 1}" => "ssh root@${node.ipv4_address}"
  }
}

output "do_start_commands" {
  description = "Commands to run do-start.sh on each node after setup"
  value = {
    for i, node in digitalocean_droplet.node :
    "node-${i + 1}" => join(" ", [
      "ssh root@${node.ipv4_address}",
      "\"cd /opt/blazil &&",
      "BLAZIL_NODE_ID=node-${i + 1} BLAZIL_SHARD_ID=${i}",
      "./scripts/do-start.sh",
      join(" ", digitalocean_droplet.node[*].ipv4_address_private),
      "\"",
    ])
  }
}

output "volume_names" {
  description = "TigerBeetle data volume names (do not delete — VSR state lives here)"
  value       = digitalocean_volume.tb_data[*].name
}

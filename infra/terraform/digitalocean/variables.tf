variable "do_token" {
  description = "DigitalOcean API token. Set via TF_VAR_do_token env var — never commit."
  type        = string
  sensitive   = true
}

variable "region" {
  description = "DigitalOcean region slug."
  type        = string
  default     = "nyc3"
}

variable "droplet_size" {
  description = <<-EOT
    Droplet size slug.
      c-8        = CPU-optimized 8vCPU/16GB (~$178/month) — recommended for benchmarks.
      s-8vcpu-16gb = General purpose 8vCPU/16GB (~$84/month) — matches v0.1 $252/month cluster.
  EOT
  type        = string
  default     = "s-8vcpu-16gb"
}

variable "node_count" {
  description = "Number of Blazil nodes. Must be 3 — TigerBeetle VSR requires exactly 3 for fault tolerance."
  type        = number
  default     = 3

  validation {
    condition     = var.node_count == 3
    error_message = "TigerBeetle VSR requires exactly 3 nodes (2/3 ack quorum)."
  }
}

variable "ssh_public_key_path" {
  description = "Path to SSH public key for droplet access."
  type        = string
  default     = "~/.ssh/id_ed25519.pub"
}

variable "image" {
  description = "Droplet OS image slug. Ubuntu 22.04 required for kernel 5.15 + io_uring support."
  type        = string
  default     = "ubuntu-22-04-x64"
}

variable "project_name" {
  description = "DigitalOcean project name for resource grouping in the DO console."
  type        = string
  default     = "blazil"
}

variable "vpc_cidr" {
  description = "Private VPC IP range. Used to restrict TigerBeetle VSR and engine ports to internal traffic only."
  type        = string
  default     = "10.10.0.0/24"
}

variable "tb_data_volume_size_gb" {
  description = "TigerBeetle data volume size in GB. TB pre-allocates the full file — size once, never shrink."
  type        = number
  default     = 50
}

variable "management_cidr" {
  description = "CIDR allowed SSH access. Restrict to your IP in production. Default allows all (for initial setup only)."
  type        = string
  default     = "0.0.0.0/0"
}

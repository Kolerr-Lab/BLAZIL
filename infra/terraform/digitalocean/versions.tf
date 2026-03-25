terraform {
  required_version = ">= 1.5"

  required_providers {
    digitalocean = {
      source  = "digitalocean/digitalocean"
      version = "~> 2.36"
    }
  }

  # Uncomment to store state on DO Spaces (recommended for team use):
  # backend "s3" {
  #   endpoint                    = "https://nyc3.digitaloceanspaces.com"
  #   region                      = "us-east-1"
  #   bucket                      = "blazil-terraform-state"
  #   key                         = "blazil/terraform.tfstate"
  #   skip_credentials_validation = true
  #   skip_metadata_api_check     = true
  # }
}

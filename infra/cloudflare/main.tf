terraform {
  required_version = ">= 1.8.0"

  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
  }

  # Supply bucket, endpoint and credential-file settings privately at init time.
  # Never share a state key with the bmux or bcode stacks.
  backend "s3" {
    key                         = "cloudflare/gchatui-site.tfstate"
    region                      = "auto"
    skip_credentials_validation = true
    skip_metadata_api_check     = true
    skip_region_validation      = true
    skip_requesting_account_id  = true
    use_path_style              = true
  }
}

provider "cloudflare" {
  api_token = trimspace(file(var.cloudflare_api_token_file))
}

variable "cloudflare_api_token_file" {
  description = "Absolute path to a private file containing the Cloudflare API token."
  type        = string
  sensitive   = true
}

variable "cloudflare_zone_id" {
  description = "Existing bmux.dev zone ID, supplied through private local configuration."
  type        = string
  sensitive   = true
}

resource "cloudflare_dns_record" "site" {
  zone_id = var.cloudflare_zone_id
  name    = "gchatui.bmux.dev"
  type    = "A"
  content = "192.0.2.1"
  ttl     = 1
  proxied = true
  comment = "Proxied placeholder address for the gchatui static-site Worker route."
}

resource "cloudflare_workers_route" "site" {
  zone_id = var.cloudflare_zone_id
  pattern = "gchatui.bmux.dev/*"
  script  = "gchatui-site"
}

output "homepage_url" {
  value = "https://gchatui.bmux.dev/"
}

output "privacy_policy_url" {
  value = "https://gchatui.bmux.dev/privacy"
}

variable "aws_account_id" {
  type = string
}
variable "base_domain" {
  type = string
}
variable "cloudflare_account_id" {
  type = string
}
variable "cloudflare_zone_id" {
  type = string
}
variable "neon_project_id" {
  type        = string
  description = "Inventory guard only; database credentials remain out of state."
}
variable "lambda_image_uri" {
  type = string
}
variable "github_repository" {
  type = string
}
variable "github_oidc_provider_arn" {
  type = string
}
variable "budget_notification_email" {
  type = string
}
variable "auth_limit_hmac_key_generation" {
  type    = number
  default = 1
}

check "staging_inventory" {
  assert {
    condition     = length(trimspace(var.neon_project_id)) > 0
    error_message = "staging requires its own Neon project ID"
  }
}

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
  description = "Inventory guard only; must identify the dedicated production Neon project."
}
variable "staging_aws_account_id" {
  type        = string
  description = "Staging inventory ID used only to reject account reuse."
}
variable "staging_neon_project_id" {
  type        = string
  description = "Staging inventory ID used only to reject Neon project reuse."
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
check "production_isolation" {
  assert {
    condition = (
      can(regex("^[0-9]{12}$", var.staging_aws_account_id)) &&
      var.aws_account_id != var.staging_aws_account_id &&
      length(trimspace(var.neon_project_id)) > 0 &&
      length(trimspace(var.staging_neon_project_id)) > 0 &&
      var.neon_project_id != var.staging_neon_project_id
    )
    error_message = "production requires an AWS account and Neon project distinct from staging"
  }
}

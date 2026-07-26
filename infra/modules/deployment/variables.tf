variable "project_name" {
  type    = string
  default = "taskveil"
}

variable "environment" {
  type = string
  validation {
    condition     = contains(["staging", "production"], var.environment)
    error_message = "environment must be staging or production"
  }
}

variable "aws_account_id" {
  type = string
  validation {
    condition     = can(regex("^[0-9]{12}$", var.aws_account_id))
    error_message = "aws_account_id must be a 12 digit account ID"
  }
}

variable "aws_region" {
  type    = string
  default = "eu-central-1"
  validation {
    condition     = var.aws_region == "eu-central-1"
    error_message = "the initial deployment region must be eu-central-1"
  }
}

variable "base_domain" {
  type = string
}

variable "cloudflare_zone_id" {
  type = string
}

variable "cloudflare_account_id" {
  type        = string
  description = "Cloudflare account containing the environment-specific Worker service."
}

variable "lambda_image_uri" {
  type        = string
  description = "Bootstrap ECR image URI pinned by sha256 digest. Tags are not accepted."
  validation {
    condition = can(regex(
      "^${var.aws_account_id}\\.dkr\\.ecr\\.${var.aws_region}\\.amazonaws\\.com/${var.project_name}-${var.environment}-server@sha256:[0-9a-f]{64}$",
      var.lambda_image_uri,
    ))
    error_message = "lambda_image_uri must use this environment's bootstrap ECR repository and a sha256 digest"
  }
}

variable "github_repository" {
  type        = string
  description = "GitHub owner/repository allowed to assume the staging deploy role."
}

variable "github_oidc_provider_arn" {
  type        = string
  description = "OIDC provider ARN created during the human-approved account bootstrap."
}

variable "enable_github_deploy_role" {
  type    = bool
  default = false
}

variable "budget_notification_email" {
  type        = string
  description = "Non-secret operations address; provide outside Git."
}

variable "budget_limit_usd" {
  type    = number
  default = 5
}

variable "lambda_memory_mb" {
  type    = number
  default = 512
}

variable "lambda_timeout_seconds" {
  type    = number
  default = 30
}

variable "lambda_reserved_concurrency" {
  type    = number
  default = 10
}

variable "auth_limit_hmac_key_generation" {
  type        = number
  default     = 1
  description = "Non-secret positive generation paired with the auth limit HMAC key."
  validation {
    condition     = var.auth_limit_hmac_key_generation >= 1 && floor(var.auth_limit_hmac_key_generation) == var.auth_limit_hmac_key_generation
    error_message = "auth_limit_hmac_key_generation must be a positive integer"
  }
}

variable "log_retention_days" {
  type    = number
  default = 14
}

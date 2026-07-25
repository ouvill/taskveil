locals {
  name             = "${var.project_name}-${var.environment}"
  api_domain       = "api.${var.environment}.${var.base_domain}"
  realtime_domain  = "realtime.${var.environment}.${var.base_domain}"
  realtime_service = "${var.project_name}-realtime-${var.environment}"
  lambda_name      = "${local.name}-server"
  runtime_secret   = "${local.name}/runtime"
  migration_secret = "${local.name}/migration"
  provider_secret  = "${local.name}/deployment-provider"
  deploy_oidc_sub  = "repo:${var.github_repository}:environment:${var.environment}"
  authenticated_routes = toset([
    "POST /v1/auth/register/start",
    "POST /v1/auth/register/finish",
    "POST /v1/auth/login/start",
    "POST /v1/auth/login/finish",
    "POST /v1/auth/device/certify",
    "POST /v1/auth/token",
    "POST /v1/auth/revoke",
    "POST /v1/auth/key-wrappers",
  ])
  tags = {
    Project     = var.project_name
    Environment = var.environment
    ManagedBy   = "OpenTofu"
  }
}

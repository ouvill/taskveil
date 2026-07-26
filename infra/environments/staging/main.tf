module "deployment" {
  source = "../../modules/deployment"

  environment                    = "staging"
  aws_account_id                 = var.aws_account_id
  base_domain                    = var.base_domain
  cloudflare_account_id          = var.cloudflare_account_id
  cloudflare_zone_id             = var.cloudflare_zone_id
  lambda_image_uri               = var.lambda_image_uri
  github_repository              = var.github_repository
  github_oidc_provider_arn       = var.github_oidc_provider_arn
  enable_github_deploy_role      = true
  budget_notification_email      = var.budget_notification_email
  auth_limit_hmac_key_generation = var.auth_limit_hmac_key_generation
  budget_limit_usd               = 5
}

output "deployment" {
  value = {
    api_base_url           = module.deployment.api_base_url
    ecr_repository_name    = module.deployment.ecr_repository_name
    ecr_repository_url     = module.deployment.ecr_repository_url
    lambda_function_name   = module.deployment.lambda_function_name
    lambda_alias           = module.deployment.lambda_alias
    secret_arns            = module.deployment.secret_arns
    github_deploy_role_arn = module.deployment.github_deploy_role_arn
  }
}

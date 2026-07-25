resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/${local.lambda_name}"
  retention_in_days = var.log_retention_days
  tags              = local.tags
}

resource "aws_lambda_function" "server" {
  function_name                  = local.lambda_name
  role                           = aws_iam_role.lambda.arn
  package_type                   = "Image"
  image_uri                      = var.lambda_image_uri
  architectures                  = ["x86_64"]
  memory_size                    = var.lambda_memory_mb
  timeout                        = var.lambda_timeout_seconds
  reserved_concurrent_executions = var.lambda_reserved_concurrency
  publish                        = true

  environment {
    variables = {
      TASKVEIL_RUNTIME_SECRET_ID             = data.aws_secretsmanager_secret.runtime.arn
      TASKVEIL_BILLING_ENVIRONMENT           = var.environment == "staging" ? "sandbox" : "production"
      TASKVEIL_AUTH_ISSUER                   = "https://${local.api_domain}"
      PARAMETERS_SECRETS_EXTENSION_HTTP_PORT = "2773"
      PARAMETERS_SECRETS_EXTENSION_LOG_LEVEL = "ERROR"
      SECRETS_MANAGER_TTL                    = "300"
      PORT                                   = "8080"
      RUST_LOG                               = "info,taskveil_server=info"
    }
  }

  depends_on = [aws_cloudwatch_log_group.lambda, aws_iam_role_policy.lambda]
  tags       = local.tags

  lifecycle {
    ignore_changes = [image_uri]
  }
}

resource "aws_lambda_alias" "live" {
  name             = var.environment
  description      = "Promoted ${var.environment} server version"
  function_name    = aws_lambda_function.server.function_name
  function_version = aws_lambda_function.server.version

  lifecycle {
    ignore_changes = [function_version]
  }
}

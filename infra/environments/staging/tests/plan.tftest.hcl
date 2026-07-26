mock_provider "aws" {
  mock_data "aws_iam_policy_document" {
    defaults = { json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}" }
  }
  mock_data "aws_secretsmanager_secret" {
    defaults = { arn = "arn:aws:secretsmanager:eu-central-1:111111111111:secret:test" }
  }
  mock_resource "aws_cloudwatch_log_group" {
    defaults = { arn = "arn:aws:logs:eu-central-1:111111111111:log-group:test" }
  }
  mock_resource "aws_acm_certificate" {
    defaults = { arn = "arn:aws:acm:eu-central-1:111111111111:certificate/00000000-0000-0000-0000-000000000000" }
  }
  mock_resource "aws_iam_role" {
    defaults = { arn = "arn:aws:iam::111111111111:role/taskveil-test" }
  }
  mock_data "aws_cloudwatch_event_connection" {
    defaults = { arn = "arn:aws:events:eu-central-1:111111111111:connection/taskveil-test/00000000-0000-0000-0000-000000000000" }
  }
  mock_resource "aws_cloudwatch_event_api_destination" {
    defaults = { arn = "arn:aws:events:eu-central-1:111111111111:api-destination/taskveil-test/00000000-0000-0000-0000-000000000000" }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn        = "arn:aws:lambda:eu-central-1:111111111111:function:taskveil-test-server"
      invoke_arn = "arn:aws:apigateway:eu-central-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-central-1:111111111111:function:taskveil-test-server/invocations"
      version    = "1"
    }
  }
  mock_resource "aws_apigatewayv2_api" {
    defaults = { execution_arn = "arn:aws:execute-api:eu-central-1:111111111111:test" }
  }
  mock_data "aws_ecr_repository" {
    defaults = {
      arn            = "arn:aws:ecr:eu-central-1:111111111111:repository/taskveil-test-server"
      repository_url = "111111111111.dkr.ecr.eu-central-1.amazonaws.com/taskveil-test-server"
    }
  }
}
mock_provider "cloudflare" {}

run "staging_plan" {
  command = plan

  variables {
    aws_account_id            = "111111111111"
    base_domain               = "example.invalid"
    cloudflare_account_id     = "test-account"
    cloudflare_zone_id        = "test-zone"
    neon_project_id           = "test-staging-neon-project"
    lambda_image_uri          = "111111111111.dkr.ecr.eu-central-1.amazonaws.com/taskveil-staging-server@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    github_repository         = "owner/taskveil"
    github_oidc_provider_arn  = "arn:aws:iam::111111111111:oidc-provider/token.actions.githubusercontent.com"
    budget_notification_email = "operations@example.invalid"
  }

  assert {
    condition     = output.deployment.api_base_url == "https://api.staging.example.invalid"
    error_message = "staging API domain must include the environment boundary"
  }

  assert {
    condition     = output.deployment.lambda_alias == "staging"
    error_message = "staging must publish through the staging alias"
  }
}

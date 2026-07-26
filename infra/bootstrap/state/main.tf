locals {
  name = "taskveil-${var.environment}"
  tags = {
    Project     = "taskveil"
    Environment = var.environment
    ManagedBy   = "OpenTofu-bootstrap"
  }
}

resource "aws_s3_bucket" "state" {
  bucket = var.state_bucket_name
  tags   = local.tags
  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_versioning" "state" {
  bucket = aws_s3_bucket.state.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "state" {
  bucket = aws_s3_bucket.state.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "state" {
  bucket                  = aws_s3_bucket.state.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_ecr_repository" "server" {
  name                 = "${local.name}-server"
  image_tag_mutability = "IMMUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }

  tags = local.tags
  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_ecr_lifecycle_policy" "server" {
  repository = aws_ecr_repository.server.name
  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Keep the ten most recent images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 10
      }
      action = { type = "expire" }
    }]
  })
}

resource "aws_secretsmanager_secret" "runtime" {
  name                    = "${local.name}/runtime"
  description             = "Taskveil non-owner runtime, realtime, and resync-token configuration; value is inserted out of band"
  recovery_window_in_days = 30
  tags                    = local.tags
  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_secretsmanager_secret" "migration" {
  name                    = "${local.name}/migration"
  description             = "Taskveil owner migration configuration; value is inserted out of band"
  recovery_window_in_days = 30
  tags                    = local.tags
  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_secretsmanager_secret" "deployment_provider" {
  name                    = "${local.name}/deployment-provider"
  description             = "Taskveil deployment provider configuration; value is inserted out of band"
  recovery_window_in_days = 30
  tags                    = local.tags
  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_iam_openid_connect_provider" "github" {
  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = ["6938fd4d98bab03faadb97b34396831e3780aea1"]
  tags            = local.tags
}

data "aws_iam_policy_document" "infra_assume" {
  statement {
    actions = ["sts:AssumeRoleWithWebIdentity"]
    principals {
      type        = "Federated"
      identifiers = [aws_iam_openid_connect_provider.github.arn]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:${var.github_repository}:environment:${var.environment}"]
    }
  }
}

resource "aws_iam_role" "infra_apply" {
  name               = "${local.name}-github-infra-apply"
  assume_role_policy = data.aws_iam_policy_document.infra_assume.json
  tags               = local.tags
}

data "aws_iam_policy_document" "infra_apply" {
  statement {
    sid       = "ReadDeploymentProviderForOpenTofu"
    actions   = ["secretsmanager:GetSecretValue"]
    resources = [aws_secretsmanager_secret.deployment_provider.arn]
  }
  statement {
    sid       = "StateBucket"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.state.arn]
  }
  statement {
    sid = "StateObjects"
    actions = [
      "s3:GetObject", "s3:PutObject", "s3:DeleteObject",
      "s3:GetObjectVersion", "s3:PutObjectTagging",
    ]
    resources = ["${aws_s3_bucket.state.arn}/*"]
  }
  statement {
    sid = "ManagedDeploymentResources"
    actions = [
      "acm:*", "apigateway:*", "budgets:*", "ecr:*", "lambda:*",
      "logs:*",
    ]
    resources = ["*"]
  }
  statement {
    sid     = "DescribeDeploymentSecrets"
    actions = ["secretsmanager:DescribeSecret"]
    resources = [
      aws_secretsmanager_secret.runtime.arn,
      aws_secretsmanager_secret.migration.arn,
      aws_secretsmanager_secret.deployment_provider.arn,
    ]
  }
  statement {
    sid = "ManagedIdentityBoundary"
    actions = [
      "iam:CreateRole", "iam:DeleteRole", "iam:GetRole", "iam:ListRolePolicies",
      "iam:PassRole", "iam:PutRolePolicy", "iam:DeleteRolePolicy",
      "iam:GetRolePolicy", "iam:TagRole", "iam:UntagRole", "iam:UpdateAssumeRolePolicy",
    ]
    resources = ["arn:aws:iam::${var.aws_account_id}:role/${local.name}-*"]
  }
}

resource "aws_iam_role_policy" "infra_apply" {
  name   = "${local.name}-infra-apply"
  role   = aws_iam_role.infra_apply.id
  policy = data.aws_iam_policy_document.infra_apply.json
}

output "state_bucket" {
  value = aws_s3_bucket.state.id
}
output "github_oidc_provider_arn" {
  value = aws_iam_openid_connect_provider.github.arn
}
output "infra_apply_role_arn" {
  value = aws_iam_role.infra_apply.arn
}
output "ecr_repository_url" {
  value = aws_ecr_repository.server.repository_url
}
output "ecr_repository_name" {
  value = aws_ecr_repository.server.name
}
output "secret_arns" {
  value = {
    runtime             = aws_secretsmanager_secret.runtime.arn
    migration           = aws_secretsmanager_secret.migration.arn
    deployment_provider = aws_secretsmanager_secret.deployment_provider.arn
  }
}

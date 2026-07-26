data "aws_cloudwatch_event_connection" "email_dispatch" {
  name = "${local.name}-email-dispatch"
}

resource "aws_cloudwatch_event_api_destination" "email_dispatch" {
  name                             = "${local.name}-email-dispatch"
  invocation_endpoint              = "https://${local.api_domain}/internal/email/dispatch"
  http_method                      = "POST"
  invocation_rate_limit_per_second = 1
  connection_arn                   = data.aws_cloudwatch_event_connection.email_dispatch.arn
}

data "aws_iam_policy_document" "email_dispatch_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["events.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "email_dispatch" {
  name               = "${local.name}-email-dispatch"
  assume_role_policy = data.aws_iam_policy_document.email_dispatch_assume.json
  tags               = local.tags
}

data "aws_iam_policy_document" "email_dispatch" {
  statement {
    actions   = ["events:InvokeApiDestination"]
    resources = [aws_cloudwatch_event_api_destination.email_dispatch.arn]
  }
}

resource "aws_iam_role_policy" "email_dispatch" {
  name   = "invoke-email-dispatch-only"
  role   = aws_iam_role.email_dispatch.id
  policy = data.aws_iam_policy_document.email_dispatch.json
}

resource "aws_cloudwatch_event_rule" "email_dispatch" {
  name                = "${local.name}-email-dispatch"
  schedule_expression = "rate(1 minute)"
  tags                = local.tags
}

resource "aws_cloudwatch_event_target" "email_dispatch" {
  rule     = aws_cloudwatch_event_rule.email_dispatch.name
  arn      = aws_cloudwatch_event_api_destination.email_dispatch.arn
  role_arn = aws_iam_role.email_dispatch.arn

  retry_policy {
    maximum_event_age_in_seconds = 300
    maximum_retry_attempts       = 2
  }
}

resource "aws_cloudwatch_log_group" "api_access" {
  name              = "/aws/apigateway/${local.name}"
  retention_in_days = var.log_retention_days
  tags              = local.tags
}

resource "aws_apigatewayv2_api" "server" {
  name          = "${local.name}-http"
  protocol_type = "HTTP"
  tags          = local.tags
}

resource "aws_apigatewayv2_integration" "server" {
  api_id                 = aws_apigatewayv2_api.server.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_alias.live.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
  timeout_milliseconds   = 30000
  request_parameters = {
    # The application trusts this header only when the matching runtime flag
    # is enabled. Overwrite prevents a public caller from choosing its bucket.
    "overwrite:header.x-taskveil-source-ip" = "$context.identity.sourceIp"
  }
}

resource "aws_apigatewayv2_route" "default" {
  api_id    = aws_apigatewayv2_api.server.id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.server.id}"
}

resource "aws_apigatewayv2_route" "authenticated" {
  for_each  = local.authenticated_routes
  api_id    = aws_apigatewayv2_api.server.id
  route_key = each.value
  target    = "integrations/${aws_apigatewayv2_integration.server.id}"
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.server.id
  name        = "$default"
  auto_deploy = true

  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_access.arn
    format = jsonencode({
      request_id = "$context.requestId"
      route      = "$context.routeKey"
      status     = "$context.status"
      latency_ms = "$context.responseLatency"
    })
  }

  default_route_settings {
    detailed_metrics_enabled = false
    throttling_rate_limit    = 20
    throttling_burst_limit   = 40
  }

  dynamic "route_settings" {
    for_each = local.authenticated_routes
    content {
      route_key                = route_settings.value
      detailed_metrics_enabled = false
      throttling_rate_limit    = 5
      throttling_burst_limit   = 10
    }
  }

  tags = local.tags
}

resource "aws_lambda_permission" "api" {
  statement_id  = "AllowApiGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.server.function_name
  qualifier     = aws_lambda_alias.live.name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.server.execution_arn}/*/*"
}

resource "aws_acm_certificate" "api" {
  domain_name       = local.api_domain
  validation_method = "DNS"
  tags              = local.tags
  lifecycle {
    create_before_destroy = true
  }
}

resource "cloudflare_dns_record" "api_certificate" {
  for_each = {
    for option in aws_acm_certificate.api.domain_validation_options : option.domain_name => {
      name    = option.resource_record_name
      content = option.resource_record_value
      type    = option.resource_record_type
    }
  }
  zone_id = var.cloudflare_zone_id
  name    = each.value.name
  content = each.value.content
  type    = each.value.type
  ttl     = 60
  proxied = false
}

resource "aws_acm_certificate_validation" "api" {
  certificate_arn         = aws_acm_certificate.api.arn
  validation_record_fqdns = [for record in cloudflare_dns_record.api_certificate : record.name]
}

resource "aws_apigatewayv2_domain_name" "api" {
  domain_name = local.api_domain
  domain_name_configuration {
    certificate_arn = aws_acm_certificate_validation.api.certificate_arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }
  tags = local.tags
}

resource "aws_apigatewayv2_api_mapping" "api" {
  api_id      = aws_apigatewayv2_api.server.id
  domain_name = aws_apigatewayv2_domain_name.api.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "cloudflare_dns_record" "api" {
  zone_id = var.cloudflare_zone_id
  name    = local.api_domain
  content = aws_apigatewayv2_domain_name.api.domain_name_configuration[0].target_domain_name
  type    = "CNAME"
  ttl     = 60
  proxied = false
}

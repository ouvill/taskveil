mock_provider "cloudflare" {}

variables {
  base_domain           = "example.invalid"
  cloudflare_account_id = "test-account"
  cloudflare_zone_id    = "0123456789abcdef0123456789abcdef"
}

run "realtime_publish_body_guard_plan" {
  command = plan

  assert {
    condition     = module.shared_edge.realtime_publish_body_guard_contract.kind == "zone"
    error_message = "the realtime body guard must remain a zone-level ruleset"
  }

  assert {
    condition     = module.shared_edge.realtime_publish_body_guard_contract.phase == "http_request_firewall_custom"
    error_message = "the realtime body guard must own the custom WAF entrypoint"
  }

  assert {
    condition     = module.shared_edge.realtime_publish_body_guard_contract.action == "block"
    error_message = "the realtime body guard must block matching requests"
  }

  assert {
    condition = alltrue([
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "\"realtime.staging.example.invalid\""),
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "\"realtime.production.example.invalid\""),
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "http.request.method eq \"POST\""),
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "http.request.uri.path eq \"/v1/publish\""),
    ])
    error_message = "the body guard must cover both realtime hosts and only the publish endpoint"
  }

  assert {
    condition = alltrue([
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "len(http.request.headers[\"content-length\"]) ne 1"),
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "matches r\"^(0|[1-9]|[1-9][0-9]|[1-4][0-9][0-9]|50[0-9]|51[0-2])$\""),
      strcontains(module.shared_edge.realtime_publish_body_guard_contract.expression, "http.request.body.size gt 512"),
    ])
    error_message = "the body guard must enforce one canonical 0..512 Content-Length and the actual 512-byte limit"
  }
}

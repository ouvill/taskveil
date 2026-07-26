locals {
  realtime_domains = [
    "realtime.staging.${var.base_domain}",
    "realtime.production.${var.base_domain}",
  ]
  realtime_hosts_expression = join(
    " ",
    [for domain in local.realtime_domains : "\"${domain}\""],
  )
}

# A zone has only one entry point ruleset per phase. This shared state is the
# sole owner of Taskveil's zone-level custom WAF phase; environment deployment
# states must not create or mutate a second http_request_firewall_custom root.
resource "cloudflare_ruleset" "realtime_publish_body_guard" {
  zone_id     = var.cloudflare_zone_id
  name        = "Taskveil shared edge request guards"
  description = "Reject malformed or oversized staging and production server-to-Worker publish bodies before Worker execution."
  kind        = "zone"
  phase       = "http_request_firewall_custom"

  rules = [
    {
      ref         = "taskveil_realtime_publish_body_guard"
      description = "Require one canonical Content-Length from 0 through 512 and an actual body no larger than 512 bytes."
      expression = join(" ", [
        "(http.host in {${local.realtime_hosts_expression}}",
        "and http.request.method eq \"POST\"",
        "and http.request.uri.path eq \"/v1/publish\"",
        "and (len(http.request.headers[\"content-length\"]) ne 1",
        "or not any(http.request.headers[\"content-length\"][*] matches r\"^(0|[1-9]|[1-9][0-9]|[1-4][0-9][0-9]|50[0-9]|51[0-2])$\")",
        "or http.request.body.size gt 512))",
      ])
      action  = "block"
      enabled = true
    },
  ]
}

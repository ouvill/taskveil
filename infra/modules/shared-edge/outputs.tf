output "realtime_publish_body_guard_ruleset_id" {
  value = cloudflare_ruleset.realtime_publish_body_guard.id
}

output "realtime_publish_body_guard_contract" {
  value = {
    kind       = cloudflare_ruleset.realtime_publish_body_guard.kind
    phase      = cloudflare_ruleset.realtime_publish_body_guard.phase
    action     = cloudflare_ruleset.realtime_publish_body_guard.rules[0].action
    expression = cloudflare_ruleset.realtime_publish_body_guard.rules[0].expression
  }
}

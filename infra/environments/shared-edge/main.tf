module "shared_edge" {
  source = "../../modules/shared-edge"

  base_domain        = var.base_domain
  cloudflare_zone_id = var.cloudflare_zone_id
}

output "shared_edge" {
  value = {
    cloudflare_account_id                  = var.cloudflare_account_id
    realtime_publish_body_guard_ruleset_id = module.shared_edge.realtime_publish_body_guard_ruleset_id
  }
}

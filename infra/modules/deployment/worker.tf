resource "cloudflare_workers_custom_domain" "realtime" {
  account_id = var.cloudflare_account_id
  zone_id    = var.cloudflare_zone_id
  hostname   = local.realtime_domain
  service    = local.realtime_service
}

resource "cloudflare_workers_custom_domain" "auth_email" {
  account_id = var.cloudflare_account_id
  zone_id    = var.cloudflare_zone_id
  hostname   = local.email_domain
  service    = local.email_service
}

resource "cloudflare_queue" "auth_email" {
  account_id = var.cloudflare_account_id
  queue_name = "${var.project_name}-auth-email-${var.environment}"
  settings = {
    message_retention_period = 3600
  }
}

resource "cloudflare_queue" "auth_email_dlq" {
  account_id = var.cloudflare_account_id
  queue_name = "${var.project_name}-auth-email-${var.environment}-dlq"
  settings = {
    message_retention_period = 3600
  }
}

resource "cloudflare_queue_consumer" "auth_email" {
  account_id        = var.cloudflare_account_id
  queue_id          = cloudflare_queue.auth_email.queue_id
  script_name       = local.email_service
  type              = "worker"
  dead_letter_queue = cloudflare_queue.auth_email_dlq.queue_name
  settings = {
    batch_size       = 10
    max_retries      = 8
    retry_delay      = 30
    max_wait_time_ms = 1000
  }
}

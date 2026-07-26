variable "base_domain" {
  type = string
  validation {
    condition     = length(trimspace(var.base_domain)) > 0
    error_message = "base_domain must not be empty"
  }
}

variable "cloudflare_zone_id" {
  type = string
}

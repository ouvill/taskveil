# Taskveil authentication email Worker

This Worker is the provider-neutral email delivery boundary for registration
verification. The API sends only an authenticated, encrypted command; recipient
and eight-digit one-time code are decrypted immediately before Cloudflare Email
Service submission. Logs and queue payloads contain no email address or
verification code.

## Release prerequisites

Cloudflare Email Service is a paid-plan beta. Before either environment is
released, an operator must:

1. enable Email Service and verify the sending domain;
2. replace the fail-closed `verify@example.invalid` sender in
   `wrangler.jsonc`;
3. disable Email Preview retention for the sending domain;
4. provision the Worker secrets listed in `src/env.d.ts`;
5. deploy the Worker, then apply Terraform so Terraform creates the sole Queue
   consumer configuration;
6. verify delivery, retry, dead-letter, key-rotation, and redacted-log runbooks
   in staging.

`wrangler.jsonc` intentionally declares only the producer binding. Queue and
dead-letter retention and the Worker consumer are Terraform-owned to avoid
configuration drift.

## API-to-Worker wire contract

`POST /v1/enqueue` accepts at most 8192 body bytes. The Worker rejects an
oversized `Content-Length` before reading the body and also enforces the limit
while streaming a chunked body. The API authenticates the exact body with:

```text
POST
/v1/enqueue
<x-taskveil-key-id>
<x-taskveil-timestamp>
<base64url-without-padding(SHA-256(body))>
```

`x-taskveil-signature` is the unpadded base64url HMAC-SHA-256 of those UTF-8
bytes. The timestamp is a ten-digit Unix timestamp accepted within 300 seconds.
The key ID selects only the configured current or previous signing key.

The cross-runtime conformance vector is:

```text
key (base64): AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=
body: {}
timestamp: 1785024000
body digest: RBNvo1WzZ4oRRq0W9-hknpT7T8If536DEMBg9hyq_4o
signature: FKG_nYAP0pQJvfRPngo1Ewlg9t0sfyEf0hregqPoLSo
```

The Durable Object named by `delivery_id` leases ingress before Queue
submission and records it as enqueued afterwards. Concurrent replays receive a
retryable response and cannot amplify Queue writes. If Queue submission
succeeds but its response is lost, no more than one uncertain Queue write is
permitted per five-minute lease interval. The separate delivery lease applies
the same bound at Email Service submission and permanently suppresses replay
after acceptance is recorded. Ingress and delivery state are deleted by an
alarm at `not_after + 1 hour Queue retention + 5 minutes safety margin`.

The encrypted payload is exactly:

```json
{
  "recipient": "local-part@example.com",
  "otp": "12345678",
  "template": "verify-email-otp-v1"
}
```

Unknown fields, legacy verification links, and codes other than exactly eight
ASCII digits are terminally rejected. The generated plain-text message tells
the recipient to ignore it if they did not request the code.

When the delivery Durable Object reports a live lease, no Email Service attempt
has occurred. The consumer therefore publishes a delayed replacement and
acknowledges the leased Queue message instead of calling `retry()`, so lease
waiting does not consume the finite Queue delivery-attempt budget. If publishing
that replacement fails, the original message is retried to preserve
at-least-once delivery. This follows Cloudflare's documented
[`max_retries` and explicit retry behavior][queue-retries]. Both forms allowed
by [RFC 9110 `Retry-After`][retry-after]—delta-seconds and HTTP-date—are accepted
and normalized to 1–86400 seconds; malformed or missing values fall back to 300
seconds.

[queue-retries]: https://developers.cloudflare.com/queues/configuration/batching-retries/
[retry-after]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3

# Taskveil realtime Worker

This Worker is a lossy foreground wake-up provider. PostgreSQL and the existing
HTTPS push/pull API remain the only sync authority. The Worker never stores or
forwards record data, cursors, tenant UUIDs, or device UUIDs.

## Routes

- `GET /v1/connect` requires `Upgrade: websocket` and a 300-second ticket in
  `Authorization: Bearer <ticket>`.
- `POST /v1/publish` requires the ADR-019 key ID, Unix timestamp, HMAC headers,
  and exactly one canonical decimal `Content-Length` in the range `0..=512`.
  The Worker validates the header before reading, then reads the byte stream
  through a BYOB reader with a 513-byte probe ceiling. A missing, malformed,
  under-reported, over-reported, or oversized body is cancelled before HMAC
  verification and is never forwarded to a Durable Object. A valid request
  fans out exactly `{"v":1,"type":"changed"}`.

Both routes select the tenant Durable Object through
`REALTIME_CHANNELS.jurisdiction("eu").getByName(opaqueChannel)`. The test suite
sets `TEST_ONLY_UNRESTRICTED_NAMESPACE=enabled` because the pinned local
`workerd` does not implement jurisdiction restrictions. That binding is absent
from `wrangler.jsonc` and must never be configured in a deployed environment.

## Secret bindings

Deployment requires these eight secret/text bindings. Each key value is a
base64url-no-padding encoding of exactly 32 bytes; key IDs match
`[A-Za-z0-9_-]{1,32}`.

- `TICKET_KEY_CURRENT_ID` / `TICKET_KEY_CURRENT`
- `TICKET_KEY_PREVIOUS_ID` / `TICKET_KEY_PREVIOUS`
- `PUBLISH_KEY_CURRENT_ID` / `PUBLISH_KEY_CURRENT`
- `PUBLISH_KEY_PREVIOUS_ID` / `PUBLISH_KEY_PREVIOUS`

No production values belong in this repository. The values in
`vitest.config.ts` and `test/fixtures/realtime-hmac-v1.json` are intentionally
public deterministic test material.

Wranglerは`staging` / `production`の別Worker環境とversion / deploymentを管理する。Custom DomainはOpenTofuが`realtime.<environment>.<base-domain>`を対応するWorker serviceへ接続し、version upload時のCLI optionにはしない。productionのapply / deploy workflowはない。どちらの環境もWorker code内の`REALTIME_CHANNELS.jurisdiction("eu")`を維持する。

OpenTofuは同じhost/pathへCloudflare zone-level WAF custom ruleを設定し、canonical
`Content-Length: 0..512`と実body 512 bytes以下をWorker実行前にも強制する。
`http.request.body.size`と正規表現はCloudflare Enterprise planを必要とするため、
zone-level phase entrypointは専用shared-edge stateがstaging / production両hostname分を
一括所有し、環境別deployment stateは参照も変更もしない。初回shared-edge applyと
production release前にplan entitlement、`Zone WAF Write`権限、両hostでのrule active
状態を人間が確認する。WAFは多層防御であり、Workerの513-byte bounded readを
無効化・省略してはならない。

## Observability

Each connect or publish outcome emits one JSON object containing only an
allowlisted `event` field. The possible values are
`realtime_connect_succeeded`, `realtime_connect_failed`,
`realtime_publish_succeeded`, and `realtime_publish_failed`. Tickets, URLs,
opaque channel/device tags, UUIDs, request bodies, and record metadata are never
included. Provider-specific log metadata must not be treated as a place to add
those values.

## Local verification

Use Node.js 24.18.0 as pinned by `.node-version`.

```sh
npm ci
npm run typecheck
npm test
npm run build
```

`npm run build` invokes `wrangler deploy --dry-run`; it does not deploy.

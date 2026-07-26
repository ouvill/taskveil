---
id: 019f9c34-547f-74f8-af71-c4b022b07961
title: Realtime publish bounded request body
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

Realtime Workerのserver-to-server publish endpointは、`Content-Length`がないrequestを
`request.arrayBuffer()`で全量bufferした後に512 byte上限を確認する。未認証の
chunked / HTTP/2 requestがWorker isolateのmemoryとCPUを先に消費できるため、
署名検証前のbody readを必ずboundedにする。

本作業はプロダクトオーナーが2026-07-26に承認したsecurity remediationである。
未修正の攻撃詳細は公開文書に記載しない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `SECURITY.md`
- `docs/03_技術仕様書.md`
- `docs/09_運用ガイド.md`
- `docs/tasks/PLAYBOOK.md`
- `realtime-worker/src/contracts.ts`
- `realtime-worker/src/crypto.ts`
- `realtime-worker/src/index.ts`
- `realtime-worker/test/worker.test.ts`
- `realtime-worker/README.md`
- `infra/`

一次資料:

- OWASP Denial of Service Cheat Sheet
- Cloudflare Workers Request / Streams documentation
- Fetch Standard body and stream contracts

## 3. ゴール

- `Content-Length`がない、または偽装されたrequestでも513 byteより先をbufferしない。
- server-to-server publish contractとしてcanonical `Content-Length`を必須化する。
- applicationとedge/WAFの二層でbody上限を強制する。

## 4. スコープ

### やること

- publish bodyを最大513 byteだけ読むbounded stream helperを実装する。
- canonical `Content-Length`の存在、構文、上限をbody read前に検証する。
- absent、chunked、malformed、under-reported、over-reported、oversized body testを追加する。
- server publisher、Worker README、技術仕様、infra policyを同じcontractへ合わせる。
- body streamをcancelし、raw bodyやsignatureをlogへ出さない。

### やらないこと

- Realtime channel protocol、ticket形式、HMAC suiteの変更。
- 一般clientからのpublish許可。
- Security Advisoryの公開またはCVE申請。

## 5. 実装手順

1. callerがcanonical `Content-Length`を送ることを確認する。
2. header不在・非canonical・512 byte超をread前に拒否する。
3. streamを513 byteまで読み、headerとの実長不一致と超過を拒否してcancelする。
4. Worker runtimeのstream/chunk境界を模擬するadversarial testを追加する。
5. edge/WAF ruleをinfraで固定し、application limitの代替ではなく多層防御とする。
6. 統合HEADでquality gateと独立security reviewを行う。

## 6. 受け入れ基準

- [x] `request.arrayBuffer()`による無制限buffering経路がない。
- [x] `Content-Length`不在、非canonical、負数、512超をbody read前に拒否する。
- [x] headerが512以下でも実bodyが超過または長さ不一致なら513 byte以内で拒否する。
- [x] oversized streamはcancelされ、Durable Objectへ転送されない。
- [x] 512 byte以下の正規署名付きpublishは従来どおり204になる。
- [x] application limitとedge/WAF limitの両方を構成・文書化する。
- [x] failure responseとobservabilityがbody内容や認証情報を漏らさない。
- [x] Worker test、typecheck、dry-run、infra test、独立検証が成功する。

## 7. 制約・注意事項

- `Content-Length`だけを信用せず、実際のstreamもboundedに検証する。
- 全bodyを読んでからsliceする実装は採用しない。
- malformed requestの詳細をunauthenticated callerへ返さない。
- 修正の公開は品質ゲートと独立検証の完了後にcoordinated disclosure手順で行う。

## 8. 完了報告に含めるべき内容

- header contractとbounded read algorithm
- stream cancel・length mismatchの扱い
- edge/WAF多層防御
- adversarial testと最大read量の証拠
- quality gate、独立検証、公開準備の結果
- Cloudflare productionで残る人間確認

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果:
  - Worker publish ingressを`request.arrayBuffer()`からBYOB bounded readerへ置換した。
    canonical decimal `Content-Length`を必須とし、header不在、非canonical、負数、
    512超をstream read前に拒否する。headerが有効でも実streamを最大513 bytesだけ
    要求し、実長不一致または超過時は残streamをcancelしてHMAC検証とDurable Object
    転送を行わない。
  - Rust publisherは署名対象raw bodyのbyte長を明示的なcanonical
    `Content-Length`として送る。実HTTP request testでbody、header、signatureを
    同時に確認した。
  - Cloudflare zoneの`http_request_firewall_custom` phaseは1 entrypointだけという
    ownership制約に合わせ、環境別deployment stateからWAFを分離した。
    `infra/environments/shared-edge`と`infra/modules/shared-edge`を正本とし、
    staging / production両realtime hostについてContent-Lengthの個数・canonical
    0..512形式・実body 512 bytes以下をWorker実行前にもblockする。
  - Worker README、infra README、技術仕様、運用ガイドを同じcontractへ更新した。
    failure responseは既存のgeneric 401、observabilityは既存allowlist eventだけを
    維持し、body、signature、opaque identifierを追加していない。
- adversarial test:
  - byte-oriented custom streamで、Content-Length不在、先頭0、負数、小数、512超を
    read 0 byteで拒否・cancelすることを確認した。
  - under-reportedと実body oversizedを513 bytesで停止・cancelし、over-reportedを
    実長不一致として拒否することを確認した。
  - exact 512-byte streamのbounded readと、正規署名付きproduction形publishの204を
    確認した。
- 品質ゲート:
  - Node.js 24.18.0 `npm run typecheck`: 成功。
  - Node.js 24.18.0 `npm test`: 15件成功。
  - Node.js 24.18.0 `npm run build`
    (`wrangler deploy --dry-run --env staging`): 成功。
  - `cargo fmt --all -- --check`: 成功。
  - `cargo clippy -p taskveil-server -- -D warnings`: 成功。
  - `cargo test -p taskveil-server realtime --lib`: 8件成功。
  - shared-edge / staging / productionの`tofu validate`: 成功。
  - shared-edge `tofu test`: WAF contract plan test 1件成功。
  - `tofu fmt -check -recursive infra`: 成功。
  - `git diff --check`: 成功。
- Commit: 本work itemを含む`fix(realtime): bound publish request bodies`
  （最終hashはGit履歴を正本とする）。
- compatibility影響:
  - `POST /v1/publish`はcanonical `Content-Length`必須となる。repository内の唯一の
    production callerは同じ変更でheaderを明示するため、現行serverとの組合せでは
    wire不整合を作らない。
- Cloudflare productionで残る人間確認:
  - shared-edge専用remote state bucketと、zone-wide entrypointだけを管理する
    最小権限credentialをGit外で用意する。
  - Cloudflare Enterprise planで`http.request.body.size`とregexが利用可能であり、
    credentialに`Zone WAF Write`があることを確認する。
  - shared-edge plan / apply後、staging / production両hostでruleがactiveであることを
    Cloudflare側で確認する。production apply / deploy workflowは本taskでは追加しない。
- 公開準備:
  - 最新の統合対象に対する全品質ゲート再実行と、承認済みの
    coordinated disclosure手順による公開が残る。

### 独立検証

- 判定: 合格（blockerなし）
- 根拠:
  - BYOB readerが不正headerをread前に拒否し、正常headerでも最大513 bytesで停止して
    reject経路をcancel、HMAC検証より前にbody契約を確定することを確認した。
  - Rust callerの明示的`Content-Length`、adversarial Worker tests、shared-edgeの
    zone-wide ownershipとWAF式を確認した。
  - 初回指摘のshared-edge CI欠落に対し、infra workflowへinit / validate / testを
    追加した。plan testがruleset kind / phase / block action、両host、publish path、
    canonical Content-Length、実body 512-byte上限を検証して成功することを再確認した。
- 検証者: quality_review（実装担当とは別エージェント）

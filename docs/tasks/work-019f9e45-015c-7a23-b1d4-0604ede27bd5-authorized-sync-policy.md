---
id: 019f9e45-015c-7a23-b1d4-0604ede27bd5
title: Authorized sync request policy
status: done
lane: standard
milestone: maintenance
---

# Authorized sync request policy

## 1. 背景とコンテキスト

sync routeではbearer解析、session / device、tenant membership、entitlement、
protocol versionの確認が各handlerへ重複している。realtime ticketも同じ
request-time policyを使う設計だが、protocol gateを共有しておらず、新route追加時の
認可漏れを構造的に検出できない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/adr/ADR-019.md`
- `server/src/auth.rs`
- `server/src/billing.rs`
- `server/src/routes/sync.rs`
- `server/src/routes/realtime.rs`
- GitHub Issue #62

## 3. ゴール

sync / realtime ticketのrequest-time authorizationを共通境界へ集約し、検証順序と
失敗時の外部応答を固定する。新routeが共通境界を通らない変更はCIで検出する。

## 4. スコープ

### やること

- `AuthorizedSyncRequest` extractorを追加する。
- bearer、session / device、tenant membership、entitlement、protocol versionの
  検証順序を一箇所へ集約する。
- 全sync handlerとrealtime ticket handlerを共通extractorへ移行する。
- 全route × 全authorization gateのnegative matrix testを追加する。
- route登録とextractor利用を照合する構造CI gateと、その負系fixtureを追加する。
- realtime ticket clientからprotocol versionを送る。

### やらないこと

- session、device、membership、entitlement自体の判定規則を変更しない。
- sync wire format、DB schema、課金provider、realtime Worker protocolを変更しない。
- 新規依存を追加しない。

## 5. 実装手順

1. tenant pathとheaderから共通policyを実行して認可済みcontextを返すextractorを作る。
2. route handlerの手書き認可をextractorへ置き換える。
3. realtime clientと既存test fixtureへprotocol headerを追加する。
4. session、device、membership、entitlement、protocolの各失敗を全routeで検証する。
5. route登録にextractorがない変更を拒否するCI scriptとfixture testを追加する。
6. 統合HEADでRust、境界script、差分品質gateを実行し、独立検証へ渡す。

## 6. 受け入れ基準

- session / device、tenant membership、entitlement、protocol versionがこの順序で
  共通policyから評価される。
- 全sync / realtime ticket routeが`AuthorizedSyncRequest`を要求する。
- 認証・membership失敗は既存どおりopaqueな401、entitlement失敗は402、
  protocol mismatchは409となり、後段の失敗を先に公開しない。
- 共通policyが返したtenant / user / device contextだけをhandlerが利用する。
- 全routeを列挙したnegative authorization matrixが合格する。
- extractorを欠くrouteを追加したfixtureで構造CI gateが失敗する。
- repository共通品質gateが合格する。

## 7. 制約・注意事項

- bearer token、tenant / device識別子をlogへ出さない。
- 認可済みtenantと別のpath / body由来tenantを処理へ渡さない。
- protocol checkより前にsession / device、membership、entitlementを評価し、
  後段の状態を未認証callerへ公開しない。
- `docs/01_企画書.md` / `docs/02_機能仕様書.md`は変更しない。

## 8. 完了報告に含めるべき内容

- 共通extractorと検証順序
- 移行したroute一覧
- negative matrixと構造gateの実行結果
- repository共通品質gateの実行結果
- 独立検証結果

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: `AuthorizedSyncRequest` extractorへbearer、session / device、tenant
  membership、entitlement、protocol versionの順序を集約した。DB側のgateは同じ
  transactionとuser / tenant RLS contextを共有し、protocol checkを最後に行う。
  全13 sync routeとrealtime ticketを共通extractorへ移行し、realtime clientも
  protocol version headerを送るようにした。
- 構造gate: 登録済みhandlerが`AuthorizedSyncRequest`を要求すること、route側へ
  手書きpolicyが復活していないこと、登録route数とnegative matrix列挙数が一致する
  ことをCIで検査する。extractor欠落と未保護新routeのfixtureが期待どおり失敗した。
- 証拠: `negative_authorization_matrix_covers_every_sync_and_realtime_route`で14 route ×
  session、revoked session、device、membership、entitlement、protocolを検証した。
  後段protocolを同時に不正化しても前段の401 / 402が先に返り、invalid bearerと
  malformed query / JSON body / device pathの組合せでもopaque 401が先行した。
- 品質gate: `cargo fmt --all -- --check`、SQLx offline all-target check、
  `cargo clippy --workspace --all-targets -- -D warnings`、Docker-backed
  `cargo test --workspace`、`./tool/sqlx_prepare.sh --check`、client / protocol /
  authorized-sync boundary checkと負系fixture、ADR structure、`git diff --check`が成功した。
- Commit: `75a4263`、`f416d15`
- 未解決: なし。

### 独立検証

- 判定: APPROVE（P0〜P3の未解決findingなし）
- 根拠: 親エージェントが全14 routeのhandler signature、共通policyの
  session / device → membership → entitlement → protocol順序、認可済みtenant /
  actorの利用、realtime client header、構造gateの正負fixtureを独立確認した。
  初回レビューでQuery / JSON / device path extractorが認可より先行する箇所を検出し、
  invalid bearerとの組合せ5件でopaque 401が先行するよう修正した。またPostgreSQL
  READ COMMITTEDを単一snapshotと誤記しないよう、同一transaction / RLS contextの
  保証へ文言を修正した。最終HEADで構造gateとDocker-backed全route negative
  authorization matrixを再実行し、成功を確認した。
- 検証者: Codex親エージェント

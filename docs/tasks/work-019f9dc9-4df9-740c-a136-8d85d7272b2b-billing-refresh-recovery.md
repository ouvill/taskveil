---
id: 019f9dc9-4df9-740c-a136-8d85d7272b2b
title: Recoverable billing refresh state
status: active
lane: standard
milestone: maintenance
---

## 1. 背景

GitHub Issue #64では、billing refreshの一時失敗でprovider全体が
`AsyncError`へ遷移し、`state.value == null`による早期returnから回復不能になる。
課金状態の表示snapshotとrefresh operationの状態を分離し、最後のserver-issued
entitlementを保持したまま再取得できるようにする。

本作業はtyped FRB error候補
`1c4007b6192bcc9a848a07194e2614c5167104b1`を基点とする。

## 2. ゴール

- success → transient failure → recoveryで最後の正常値を失わない。
- stale表示と安全なtyped refresh errorをstateで明示する。
- foreground resume、network recovery、manual retryを同じrefreshへ集約する。
- concurrent refreshをsingle-flight化する。

## 3. 受け入れ基準

- [ ] refresh失敗後も`AsyncData`と最後の正常なentitlementを保持する。
- [ ] stale snapshotであることとrefresh errorをUIで明示する。
- [ ] unknown例外のraw文字列をUIへ表示しない。
- [ ] success → failure → recoveryのprovider testが通る。
- [ ] concurrent refreshがserver call 1回へsingle-flight化される。
- [ ] foreground resume、realtime接続復旧、manual retryから再取得できる。
- [ ] Flutter analyze/test、hardcoded string/boundary gateが通る。
- [ ] 独立検証でP1/P2相当の未解決指摘がない。

## 4. 制約

- cached/stale entitlementは表示用であり、同期認可の正本にしない。
- server request時のentitlement認可を迂回しない。
- provider/store例外や内部文字列をユーザー向け文言へ流さない。
- refresh失敗を購入失敗と混同しない。

## 5. 完了報告

- 実装後に記録する。

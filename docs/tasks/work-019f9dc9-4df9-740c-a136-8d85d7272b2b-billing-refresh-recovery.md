---
id: 019f9dc9-4df9-740c-a136-8d85d7272b2b
title: Recoverable billing refresh state
status: done
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

- [x] refresh失敗後も`AsyncData`と最後の正常なentitlementを保持する。
- [x] stale snapshotであることとrefresh errorをUIで明示する。
- [x] unknown例外のraw文字列をUIへ表示しない。
- [x] success → failure → recoveryのprovider testが通る。
- [x] concurrent refreshがserver call 1回へsingle-flight化される。
- [x] foreground resume、realtime接続復旧、manual retryから再取得できる。
- [x] Flutter analyze/test、hardcoded string/boundary gateが通る。
- [x] 独立検証でP1/P2相当の未解決指摘がない。

## 4. 制約

- cached/stale entitlementは表示用であり、同期認可の正本にしない。
- server request時のentitlement認可を迂回しない。
- provider/store例外や内部文字列をユーザー向け文言へ流さない。
- refresh失敗を購入失敗と混同しない。

## 5. 完了報告

- 課金snapshotをrefresh operationから分離し、失敗時も最後のserver-issued
  entitlementを`AsyncData`のまま保持する。stale状態とtyped errorは明示し、未知例外は
  fixed `internal` codeへ閉じる。
- refresh、purchase、restoreは共通FIFOへ直列化した。refreshはsingle-flight、
  store actionは二重開始を防ぎ、purchase完了後のserver refreshとnetwork/resume/manual
  refreshが互いのstateを上書きしない。
- cacheなしbootstrap失敗はmanual retryまたは最初のrealtime connected eventで
  providerを再構築する。freshな初期snapshotでは不要なserver refreshを行わない。
- provider generationと`ref.mounted`によりlogout / invalidate後の遅延完了をpublish
  しない。回帰testへlogout中refresh、明示invalidate、購入中refreshを追加した。
- process-wide RevenueCat identityを`uninitialized / closed / open` admission stateと
  epoch tokenで管理する。login / register / logout開始時に同期的に旧admissionを閉じ、
  認証成功時だけ新epochを発行する。初期化後の旧session rebuild、closed中に開始した
  catalog取得、旧accountのFIFO backlogはadmissionを再開できない。
- login / register / logout失敗時は実sessionを再照合し、同じuser / tenantなら
  admissionとbilling providerを復旧する。signed-outならその状態を反映し、identityを
  確認できない場合はtyped errorを表示してfail closedにする。
- notifierのrefresh、store action queue、single-flightをgeneration単位へ分離した。
  process-wide coordinatorだけがSDK操作をFIFO化し、旧accountの停止したnetwork
  refreshは次accountを阻害しない。logoutはclosed admissionのSDK FIFOをdrainしてから
  bridge credentialを削除するため、provider invalidate中のnative transactionも跨がない。
- native store actionの完了後は`storeTransactionBusy`を解除し、server entitlement
  refreshだけが停止してもlogoutできる。store未接続/cached-only状態ではstore操作を
  無効化してretryだけを提示する。
- Retryは標準`TextButton`の48×48以上のtap targetとbutton semanticsを維持する。
- 最新typed FRB error HEADへrebase後、最終課金provider/store対象46件、
  Flutter全331件と`flutter analyze --no-pub`がPASSした。visual QA harness 1件のみ
  intentional skip。hardcoded strings、client boundary、`git diff --check`もPASS。
- 初回独立レビューのP1 2件、P2 2件（no-cache回復、課金操作競合、破棄後publish、
  tap target）と、再レビューのidentity admission / auth failure recovery /
  cross-generation queue / in-flight logout / store readiness指摘を修正した。最終独立
  レビューはP0〜P3未解決なしで合格した。

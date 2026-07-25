# Taskveil 開発ステータス

> 更新日: 2026-07-18

UUIDv7 work item方式のpilot中である。長期計画はPhase計画書、設計判断はADR、新形式work itemの状態は各 `work-*.md` のfront matter、完了履歴はtask本文とgitを参照する。このファイルにはpilot前の進捗スナップショットと人間作業だけを残す。

## 現在

- 進行中: なし。
- 保留: なし。
- 最新の完了: **P2-M8 templates and recurring tasks** — content-only template、RRULE local settlement、UUIDv5重複防止、Tenant Root DEK同期、streak、英日Templates UIを実装し、全品質ゲートと独立検証を完了した。
- Phase 1: M1〜M4完了。M5リリース準備は課金基盤完成後まで延期する。
- Phase 2: P2-M1〜M4・M6〜M8完了。P2-M5は削除同期、macOS / iOS Simulator確認、Android Rust FFI / Flutter release APK・AAB build、Android Keystore、通知permission / snooze、Google Play購入clientを実装した。Pixel 7a / Android 16接続実機のKeystore instrumentation test、Device Key rotation 2回、プロセス再起動後のSQLCipher DB reopenも完了し、Android Emulatorで主要UI・登録・ログイン・2 profile双方向同期を自動検証する。Android接続実機の通知・同期は最終release gateとして残る。
- 一般リリースゲート: **Billing foundation release gate**。課金基盤、App Store / Google Playの各sandbox E2E、server-side entitlement、失効時認可が完了するまでstore提出、release tag、公開告知を行わない。

## UUIDv7 work item pilot

pilot対象の候補と状態は新形式work itemだけに記録する。次のコマンドで確認する。

```sh
rg -n '^status: (backlog|active|blocked)$' docs/tasks/work-*.md
```

`STATUS.md` へNext一覧を重複転記しない。pilot合格後の全面移行と、このファイルの廃止判断は別work itemで扱う。

## 人間作業・判断

- iOS実機で通知、Keychainゼロプロンプト、同期を通し確認する。
- stagingのAWS / Neon / Cloudflare bootstrap、secret投入、初回applyを人間承認付きで実行する。
- RevenueCat / App Store / Google Play商品を承認済みの契約どおりに設定し、各store sandbox E2Eを完了する。
- Android接続実機で通知、アカウント登録・ログイン、別端末との同期動作を最終確認する。

## 作業開始時に読むもの

1. `AGENTS.md`
2. 対応するPhase計画書・技術仕様・ADR
3. 対象の `work-*.md`、または既存の `task-*.md`
4. `PLAYBOOK.md`（標準・重要変更レーンの場合）
5. この `STATUS.md` / `BACKLOG.md`（pilot移行情報やlegacy候補が必要な場合だけ）

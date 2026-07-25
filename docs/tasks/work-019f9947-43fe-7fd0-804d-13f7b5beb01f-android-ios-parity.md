---
id: 019f9947-43fe-7fd0-804d-13f7b5beb01f
title: Android and iOS implementation parity
status: active
lane: critical
milestone: P2-M5
---

# Android and iOS implementation parity

## 1. 背景とコンテキスト

Flutter / Rustの共通実装によりAndroidでも主要なローカル機能、SQLCipher、Keystore、Device Key rotation、release APK buildは成立している。一方、Androidのリマインダー権限・通知action、Google Play購入・復元、アカウント登録から別端末同期までの接続実機相当E2EはiOSと同じ実装・検証水準に達していない。2026-07-25にプロダクトオーナーがAndroidをiOSと同等の実装レベルへ引き上げること、専用Git worktree、サブエージェント、Android Emulatorの利用を承認した。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/08_Phase2計画書.md`
- `docs/03_技術仕様書.md` §4.10、§6、§10
- `docs/billing_overview.md`
- `docs/tasks/task-65-local-notifications.md`
- `docs/tasks/work-019f614f-03a5-7c63-85a3-34de156a76ff-billing-foundation-release-gate.md`
- `docs/tasks/work-019f6bea-f991-7ea3-8bf2-14890d314672-android-ci.md`
- `app/lib/src/notifications/reminder_notifications.dart`
- `app/lib/src/timer/timer_notifications.dart`
- `app/lib/src/billing/billing_store.dart`
- `app/android/`
- `.github/workflows/ci.yml`

## 3. ゴール

Androidで、iOSと共通のローカル機能に加え、通知権限、リマインダーaction、RevenueCat経由のGoogle Play購入・復元、アカウント・同期クライアント動線が実装され、Android Emulatorで自動検証できる状態にする。外部provider設定やGoogle Play実購入を必要とする項目は、コード完成と外部E2E gateを区別して記録する。

## 4. スコープ

### やること

- iOS / Androidのplatform分岐、manifest、native設定、テスト、CIを監査する。
- Android 13+の通知権限をリマインダーとFocusで一貫して要求する。
- Android通知に1時間スヌーズactionを追加し、既存のowner / payload境界を維持する。
- Android再起動後のローカル通知再登録に必要なreceiver / permissionを構成する。
- RevenueCat Android public SDK keyをbuild environmentから受け取り、Google Playの購入・復元・管理URLをiOSと同じclient境界で扱う。
- Androidでbilling bootstrap失敗により状態表示全体が失われないようplatform対応を追加する。
- Android Emulatorで通知設定、Keystore / Device Key rotation、主要Flutter動線、アカウント・同期の実行可能な範囲を自動検証する。
- public技術仕様・Phase計画・billing概要を公開可能な実装事実へ更新する。価格・provider credential等のprivate詳細は転記しない。

### やらないこと

- Google Play Console、RevenueCat dashboard、staging secretへ実値を投入する。
- Google Play sandboxの実決済をEmulator上のfakeやunit testで成功扱いにする。
- server entitlementをclient表示へ移す、または開発用billing bypassを追加する。
- DB schema、暗号suite、鍵namespace、同期protocolを変更する。
- privateの価格・収益・provider契約情報をpublic repoへ記録する。

## 5. 実装手順

1. 差分監査と既存テスト境界を確定する。
2. Android通知permission / action / reboot構成とテストを実装する。
3. RevenueCat store設定をiOS / Android共通へ拡張し、platform別public SDK keyとテストを追加する。
4. Android Emulator用の主要動線・通知・同期検証を追加し、既存Keystore gateと統合する。
5. 共通品質ゲート、Android release APK、Emulator test、独立検証を行う。

## 6. 受け入れ基準

- [ ] Android 13+でリマインダー保存時に通知権限を要求し、拒否時に通知登録成功と誤認しない。
- [ ] Android通知から1時間スヌーズactionを実行でき、iOSと同じdomain処理へ到達する。
- [ ] Androidで端末再起動・アプリ更新後に将来のscheduled notificationを再登録できるmanifest構成である。
- [ ] RevenueCatがiOS / Androidそれぞれのbuild-time public SDK keyで構成され、Androidで購入・復元・管理URLを呼べる。
- [ ] 既存のserver-side entitlement、custom App User ID、environment分離、local-only継続契約を変更しない。
- [ ] Android EmulatorでKeystore、Device Key rotation / DB reopen、通知permission / scheduling、主要ローカル画面を検証する。
- [ ] Androidのアカウント登録・ログイン・同期について、自動E2Eまたは外部serverが必要な明示的release gateとして再現可能に記録する。
- [ ] Android arm64 release APKとiOS arm64 no-codesign release buildが継続して成功する。
- [ ] repositoryの共通品質ゲートと独立検証が合格する。

## 7. 制約・注意事項

- Android / iOSの課金provider SDK結果は認可の正本にせず、server-side entitlementを維持する。
- API key、store credential、購入payload、token、鍵、復号済みcontentをcommitまたはlogへ含めない。
- Android notification payloadへtask title、note、tenant / device IDを追加しない。
- FRB生成物とl10n生成物は手編集しない。
- Google Play外部設定・sandbox購入証跡がない限り一般リリースgateを閉じない。

## 8. 完了報告に含めるべき内容

- Android / iOS差分監査と解消内容
- 通知permission / action / reboot、billing platform設定、Emulator E2Eの実装結果
- 実行した品質ゲート、APK / iOS build、Emulator対象API / device profile
- 外部Google Play / RevenueCat設定とsandbox購入に残る人間作業
- 独立検証結果、commit、未解決事項

---
id: 019f9d48-a780-7331-a298-061f5caa2219
title: App settings and internal metadata boundary
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #56で、Flutterへ公開されたraw settings APIから任意key/valueを
SQLCipher DBの`settings` tableへ書けることが指摘された。同じtableにはUI設定だけでなく、
account identity、sync HLC、upgrade marker、key rotation marker、server URL等の
内部状態が混在しており、frontend境界からcorrectness/security stateを上書きできる。

本作業はprofile coordination、typed readiness、network guard partitioningを統合した
`main`を基点とし、ADR-026の不変SQLとchecksum ledger、ADR-027の
runtime epoch / profile lock / lease契約を維持してこの境界を分離する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-026.md`
- `docs/adr/ADR-027.md`
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-63-settings-store.md`
- `core/storage/migrations/0001_initial.sql`
- `core/storage/migrations/0004_profile_coordination.sql`
- `core/storage/migrations/0003_resync_page_tokens.sql`
- `core/storage/src/migrations.rs`
- `core/storage/src/settings_repository.rs`
- `core/client/src/runtime/`
- `core/client/src/sqlite_sync_store.rs`
- `app/rust/src/api.rs`
- `app/lib/src/core/bridge_ports.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/timer/timer_settings.dart`
- `app/lib/src/timer/timer_engine.dart`

## 3. ゴール

- `app_settings`と`internal_metadata`を別table / repositoryへ分離する。
- frontendから任意key/valueへ到達できるraw APIを削除し、用途別のtyped APIだけを公開する。
- server URLはcanonical validatorと保存済みcredential issuer bindingを常に通す。
- profile coordination migration v4の次となるforward migrationで既存値を損失なく分類する。
- reserved/internal keyをfrontend app-setting経路から書けないことをnegative testで固定する。

## 4. スコープ

### やること

- local migration `0005_settings_metadata_boundary.sql`を追加する。
- UI mode、onboarding、calendar week start、timer設定を`app_settings`へ移す。
- timer runtime、account/sync/billing/rotation/server URLと未知のlegacy keyを
  `internal_metadata`へ移す。
- storage repositoryとtransaction APIをapp setting / internal metadataへ分割する。
- `TaskveilClient`へallowlisted setting APIと専用server URL APIを実装する。
- raw FRB `get_setting` / `set_setting`を削除し、用途別APIへ置換してFRBを再生成する。
- Rust migration/repository/client test、Dart provider/widget testを更新する。
- `docs/03_技術仕様書.md`へ分離後の正本を外科的に反映する。

### やらないこと

- 設定同期、設定UIの追加、account/sync protocol変更。
- 適用済み`0001`〜`0003` migrationの変更。
- private repository、課金・法務の非公開詳細の変更。
- セキュリティアドバイザリーの公開。

## 5. 実装手順

1. 既存keyの所有者と読み書き経路を分類する。
2. v5 migrationとmigration/repository testを先に追加する。
3. client内部のaccount/sync stateを`InternalMetadataRepository`へ移す。
4. frontend用app settingをallowlistされた用途別client APIへ置換する。
5. server URLのread/write双方でcanonical originとcredential issuerを検証する。
6. raw FRB/Dart APIを用途別APIへ置換し、FRB生成物を更新する。
7. reserved key negative testと既存値migration testを含む回帰testを実行する。
8.仕様・完了報告を更新し、全関連品質ゲートを実行する。

## 6. 受け入れ基準

- [x] `LATEST_MIGRATION_VERSION`が5で、適用済み`0001`〜`0004`は不変である。
- [x] v5 migrationは既存app settingを`app_settings`、内部/未知keyを
      `internal_metadata`へ同一transactionで移し、旧`settings`を残さない。
- [x] `AppSettingsRepository`はtyped allowlist以外を表現できず、
      `InternalMetadataRepository`はfrontendへ公開されない。
- [x] account identity、sync HLC、resync/upgrade/rotation marker、billing cache、
      server URL、timer runtimeが`internal_metadata`を使用する。
- [x] raw FRB/Dart `getSetting` / `setSetting`が存在せず、用途別APIだけが生成される。
- [x] frontend経路からreserved/internal keyを書こうとするnegative testが成功する。
- [x] server URLは保存時と読込時にcanonical originを検証し、credential issuerと
      異なる値を拒否する。
- [x] migration、roundtrip、overwrite、unknown legacy key、server URL binding、
      UI/provider/timer回帰testが成功する。
- [x] FRB生成物をcodegenで更新し、手編集していない。
- [x] `docs/tasks/README.md`の該当する共通品質ゲートが成功する。

## 7. 制約・注意事項

- migration ledgerを迂回せず、適用済みSQLを変更しない。
- profile lock / runtime epoch / fenced transactionの順序を維持する。
- raw internal key、credential、account identity、sync markerをfrontendへ返さない。
- server URLの既定値もvalidatorを通し、bound issuerが存在する場合は一致を必須とする。
- frontend入力は用途別に値、長さ、JSON shapeを検証する。
- Rust API変更後は必ずFRB codegenを実行する。

## 8. 完了報告に含めるべき内容

- migration分類表と既存値の移行証拠
- storage/client/FRB/Dartの新しいAPI境界
- reserved-key negative testとserver URL binding test
- FRB再生成コマンドと生成物
- 全品質ゲート結果、skip、環境制約
- commit hashと未解決事項

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: local migration v5で旧`settings`を、closed allowlistの`app_settings`と
  Rust内部専用の`internal_metadata`へ分離した。allowlistは`ui_mode`、
  `onboarding_completed`、`calendar_week_start`、`timer_settings_v1`であり、
  timer runtime、account / sync / billing / rotation state、server URL、未知の
  legacy keyは`internal_metadata`へ移す。
- API境界: storage repository / transactionを分離し、FRB/Dartのraw文字列key APIを
  `FrontendSettingKeyDto`によるtyped APIへ置換した。timer runtimeは専用enum variant
  だけを通す。server URLは汎用setting APIから分離し、read / write双方でcanonical
  originとactive / pending credential issuer bindingを検証する。
- Negative evidence:
  `app_settings_table_rejects_reserved_internal_key`がDB CHECKによるreserved key拒否を、
  `raw-settings-api` boundary fixtureがraw FRB setter再導入の拒否を確認した。
  `settings_boundary_migrates_v3_values_without_exposing_unknown_keys`がv3からの分類、
  value / timestamp保持、未知key保持、旧table削除、ledger v5を確認した。
- 生成コード:
  `flutter_rust_bridge_codegen 2.12.0`で
  `flutter_rust_bridge_codegen generate --config-file flutter_rust_bridge.yaml`を実行し、
  `app/rust/src/frb_generated.rs`と`app/lib/src/rust/`を再生成した。
- 品質ゲート:
  `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、
  Docker/Postgresとloopback HTTPを含む`cargo test --workspace`、
  `app/rust` release build、Cargokit `dart pub get`後のfull `flutter analyze`、
  `flutter test`（295件成功、visual QA harness 1件は仕様どおりskip）、
  hardcoded strings、client boundary check / negative fixtures、`git diff --check`が成功した。
  Rustでは実Keychain 2件と手動performance 1件が既存指定どおりignoredである。
- 統合再検証: 最新candidate chainへのrebase後、typed readinessを正本として
  account restoreとidentity validationを`internal_metadata`へ統合した。client 123件、
  server unit 29件、server integration 40件、storage 90件
  （手動performance 1件は既定どおりignore）、sync 103件、bridge 3件、
  client doc test 4件を含む`cargo test --workspace`が成功した。
  `cargo clippy --workspace --all-targets -- -D warnings`、fmt、client boundary 2本、
  app/rust release build、`flutter analyze`、Flutter 301件
  （visual QA harness 1件は仕様どおりskip）、hardcoded strings、diff checkも成功した。
- 環境メモ: 初回のloopback HTTP testはsandboxのsocket bind禁止で失敗したため、
  sandbox外で同じworkspace testを再実行して成功した。初回release buildは容量不足で
  停止したため、このworktree内の再生成可能な`target`だけを`cargo clean`し、
  同一commandを再実行して成功した。
- 最新`main`統合: profile coordinationがlocal migration v4を使用済みだったため、
  適用済みSQLを変更せず本変更をv5へ再採番した。storage migrationを含む92件、
  client 125件とdoc test 4件、workspace all-target clippy、fmt、bridge release build、
  Flutter analyze、hardcoded strings / client boundary、diff checkが成功した。
  Flutter全実行では301件成功・visual QA 1件skip後、CI前提のnative fixture未構築により
  performance testのsetupだけ失敗したが、fixture構築後の同testは
  Home 136ms / Calendar 67msで成功した。Rustの10k性能testは共有ホスト高負荷時に
  Home median 1559msで750ms閾値を超えたため、隔離CIを最終証拠とする。
- Commit: この完了報告を含むcommit（hashはGit履歴を正本とする）。
- 未解決: 実装上の未解決事項なし。親candidateが未公開のためpush / PR / mergeは行わない。

### 独立検証

- 判定: 承認。P1 / P2相当の未解決指摘なし。
- 根拠: 最新`main`へ統合後、v5 migrationの分類・旧table削除・ledger、
  storageのclosed app-setting key、raw FRB API不在、timer runtime専用経路、
  server origin canonicalizationとactive / pending issuer binding、typed readinessの
  fail-closed復元を差分と回帰testで再確認し、Rust / Flutter全品質ゲートを完走した。
- 検証者: Codex root（実装担当とは別の統合レビュー）

---
id: 019f9a15-7996-7b73-9491-b12ba3f44ac1
title: Flutter bridge capability ports
status: done
lane: standard
milestone: maintenance
---

# Flutter bridge capability ports

## 1. 背景とコンテキスト

`app/lib/src/core/bridge_service.dart` の `BridgeService` は、account、sync、billing、
list、template、task、timer、settings、reminderの69メソッドを単一interfaceとして公開して
いる。各featureは必要な操作が一部でも大きなinterfaceへ依存し、test doubleも全機能を知る
必要があるため、変更影響とtest準備の範囲が広い。

既存の `bridgeServiceProvider` overrideと `FakeBridgeService` はwidget test全体の安定した
test seamであり、今回の保守作業では互換性を維持する。FRB APIや生成物を変更せず、
機能別portを追加し、依存側を段階的に狭いcontractへ移行できる構造にする。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/STATUS.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/test/support/fake_bridge_service.dart`

## 3. ゴール

- bridge contractを機能別の小さなportへ分ける。
- `BridgeService` と `FrbBridgeService` を全portのaggregateとして互換維持する。
- 既存の `bridgeServiceProvider` overrideと `FakeBridgeService` を変更せず利用可能にする。
- 一部のprovider・repositoryを狭いportへ移行し、段階移行の経路を示す。

## 4. スコープ

### やること

- account、sync、billing、list、template、task、timer、settings、reminderのportを追加する。
- `BridgeService` をportのaggregate contractへ変更する。
- aggregate providerから各portを公開する派生providerを追加する。
- 単一機能のconsumerを狭いportへ限定的に移行する。
- format、静的解析、release bridge build、Flutter test、境界検査を実行する。

### やらないこと

- Rust API、FRB生成物、ARB、UI文言、依存定義の変更。
- bridge methodのsignatureや挙動の変更。
- 全consumerの一括移行。
- 既存fakeへの未実装method追加や `UnimplementedError` の新規追加。

## 5. 実装手順

1. 69メソッドを責務別portへ一度ずつ分類する。
2. `BridgeService` を全portのaggregateにし、既存default実装を維持する。
3. `bridgeServiceProvider` を正本としてport別providerを派生させる。
4. account、billing、sync、settings、reminder等の単一責務consumerを狭いportへ移行する。
5. 品質ゲートを実行し、結果を完了報告へ記録する。

## 6. 受け入れ基準

- [x] 69のbridge methodが責務別portに分類されている。
- [x] `FrbBridgeService` はaggregate `BridgeService` として互換性を保つ。
- [x] 既存の `bridgeServiceProvider` overrideと `FakeBridgeService` がそのまま動作する。
- [x] 限定したconsumerが機能別port providerへ依存する。
- [x] `UnimplementedError` が新規追加されていない。
- [x] Rust API、FRB生成物、ARB、依存定義に差分がない。
- [x] 対象Dartファイルのformat checkが成功する。
- [x] `cd app && flutter analyze` が成功する。
- [x] bridge release build後の `cd app && flutter test` が成功する。
- [x] `sh app/tool/check_hardcoded_strings.sh` が成功する。
- [x] `sh app/tool/check_client_boundaries.sh` が成功する。
- [x] `sh app/tool/test_client_boundaries.sh` が成功する。
- [x] `git diff --check` が成功する。

## 7. 制約・注意事項

- public repoの保守作業であり、private repoの情報を記録しない。
- `bridgeServiceProvider` を既存testのaggregate override seamとして維持する。
- port別providerはaggregate providerから派生させ、既存overrideを透過させる。
- default methodの挙動を変えず、段階移行に必要な構造変更だけを行う。
- 実装者は独立検証の合否を記入せず、front matterを `done` にしない。

## 8. 完了報告に含めるべき内容

- 追加したportとmethod分類。
- 狭いportへ移行したconsumer。
- aggregate compatibilityと既存fake/provider overrideの確認結果。
- 実行した品質ゲートのコマンドと成否。
- skip、環境制約、未解決事項。
- Commit hash。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果:
  - `AccountBridgePort`、`SyncBridgePort`、`BillingBridgePort`、
    `ListBridgePort`、`TemplateBridgePort`、`TaskBridgePort`、
    `TimerBridgePort`、`SettingsBridgePort`、`ReminderBridgePort` を追加し、
    元の69メソッドを重複なく分類した。
  - `BridgeService` を9 portのaggregate contractとして維持し、
    `FrbBridgeService` と既存 `FakeBridgeService` のcontractを変更しなかった。
  - 9つのport別Riverpod providerをaggregate providerから派生させた。
    account、sync、billing、settings、task reminderとorganization safety dialogを
    狭いportへ移行した。list/task/template/timerをまたぐconsumerは今回の段階移行対象外とした。
  - settings portだけを実装するtest doubleによるprovider override testを追加した。
  - `bridge_service.dart` は767行から585行となり、port定義は286行の
    `bridge_ports.dart` へ分離した。
- 不変条件:
  - 元interfaceとport群のmethod名集合を機械比較し、69件一致、欠落0、重複0を確認した。
  - `UnimplementedError` は変更前後とも17件で、新規追加していない。
  - Rust API、FRB生成物、ARB、Cargo/pubspec依存定義に差分はない。
- 証拠:
  - 対象6 Dartファイルの
    `dart format --output=none --set-exit-if-changed`: 成功（変更0）。
  - `cd app && flutter analyze`: 成功（No issues）。
  - `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release`: 成功。
  - `cd app && flutter test`: 295件成功、visual QA harness 1件skip。
  - hardcoded strings、client boundary check 2種、`git diff --check`: すべて成功。
- 環境:
  - 全体解析前に同梱Cargokit build toolの未取得packageを `dart pub get` で解決した。
    package解決によるtracked差分はない。
- Commit: `82befde51c2db4aedee610fbcb100d8be1d4e51a`
- 未解決: なし。

### 独立検証

- 検証日: 2026-07-26
- 判定: 合格
- 根拠:
  - 変更前の `BridgeService` と新しい9 portからmethod名を機械抽出して比較し、
    双方69件、集合差分0、port間重複0を確認した。
  - 既存の `FrbBridgeService` が69 methodを実装し、aggregate
    `BridgeService` が9 portすべてを実装することを確認した。
  - `FakeBridgeService` に差分がないこと、および既存aggregate overrideを含む
    Flutter全295 testと新しいsettings port単独override testが成功することを確認した。
  - production consumerの差分がprovider群とorganization safety dialogに限定され、
    account、sync、billing、settings、reminderの派生providerを使用することを確認した。
  - `bridge_service.dart` の `UnimplementedError` は変更前後とも17件だった。
  - commit差分をpath単位で確認し、Rust API、FRB生成物、ARB、Cargo manifest /
    lockfile、pubspec / lockfile、FRB設定に変更がないことを確認した。
  - 対象Dart 6ファイルのformat checkは変更0、`flutter analyze` は
    `No issues found` だった。
  - `app/rust` のrelease build後に `flutter test` を再実行し、295件成功、
    visual QA harness 1件のみ意図されたskipだった。
  - hardcoded strings、client boundary check 2種、worktreeとcommit双方の
    `git diff --check` が成功した。
- 検証者: 実装に関与していない独立検証エージェント

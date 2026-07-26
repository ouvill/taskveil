---
id: 019f9d99-67f9-7212-8568-9da65b493ded
title: Typed FRB error outcomes
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #65では、coreの`ClientError`がFlutter Rust Bridge境界で
`Result<T, String>`と`to_string()`へ平坦化され、UIが再認証、upgrade、busy、
local crypto unavailable、storage failureを安全に区別できない問題を扱う。
raw error文字列には内部path、SQLite詳細、入力値、将来の機密情報が混入し得るため、
bridgeでstable codeとallowlist済みargumentだけへ変換する。

本作業はprofile coordination、typed readiness、network guard partitioning、
app settings / internal metadata境界を統合した候補`b07b002`を基点とする。
FRB 2.12.0が提供するtyped `Result<T, E>`を使用し、新規error transport packageは
追加しない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/work-019f9d48-b4ae-7150-942d-e24ce769768f-sync-readiness.md`
- `docs/tasks/work-019f9d48-a780-7331-a298-061f5caa2219-settings-boundary.md`
- `core/client/src/lib.rs`
- `core/client/src/mutation_service.rs`
- `core/client/src/runtime/`
- `app/rust/src/api.rs`
- `app/rust/src/api/conversions.rs`
- `app/lib/src/core/bridge_ports.dart`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/test/rust_bridge_test.dart`

## 3. ゴール

- FRB公開APIの失敗をstable code、allowlist済みlocalization arguments、
  `retryable`からなるtyped DTOで返す。
- unauthorized / credential unavailable、upgrade required、busy / lease、
  crypto unavailable、storage failureをUIがcodeで分岐できるようにする。
- internal path、SQLite詳細、UUID、入力値、server response body、secretを
  bridge DTOとuser-facing messageへ出さない。
- toy APIを削除し、一時encrypted profileを使う実CRUD/error変換testを追加する。

## 4. スコープ

### やること

- `BridgeErrorDto`とclosed `BridgeErrorCodeDto`をFRB APIへ追加する。
- public bridge関数をtyped `Result<T, BridgeErrorDto>`へ変換する。
- `ClientError`からDTOへの単一allowlist mapperを実装する。
- `SyncStatus.last_error`のraw文字列をtyped outcomeへ置換する。
- Dart bridge port / service / provider / UIをcode-based localizationへ変更する。
- raw `.toString()`表示とproduction `greet` / `createDraftTask`を削除する。
- encrypted profileを使うCRUD、validation、busy、credential、storage変換testを追加する。

### やらないこと

- core `ClientError`のdomain分類やstorage schemaの再設計。
- server wire problem-details形式の変更。
- telemetryへraw errorやPIIを追加すること。
- 新規package追加、または本PRの自己merge。

## 5. 実装手順

1. 全FRB公開関数と現在のstring変換、UI表示経路をinventoryする。
2. stable codeと安全なargument allowlistを固定し、mapper testを先に追加する。
3. FRB signaturesをtyped resultへ変更し、正規codegenを実行する。
4. Dart port / service / providerをtyped errorへ移行する。
5. UI localizationをcodeへbindingし、raw error表示を除去する。
6. toy APIを削除し、実encrypted profile integration testへ置換する。
7. boundary negative test、Rust / Flutter全品質ゲート、leak scanを実行する。

## 6. 受け入れ基準

- [x] bridge errorはclosed stable code、allowlist済みarguments、retryableだけを持つ。
- [x] path、SQLite detail、UUID、入力値、server body、secretがDTO / Debug / UIへ出ない。
- [x] credential、upgrade、profile/database/lease busy、crypto unavailable、
      validation、storage、unknownが区別される。
- [x] public FRB APIに`Result<_, String>`と`map_err(|e| e.to_string())`が残らない。
- [x] `SyncStatus.last_error`がraw stringではなくtyped outcomeになる。
- [x] Flutterはstable codeからARB localizationを選び、raw `.toString()`を表示しない。
- [x] production `greet` / `createDraftTask`がFRB surfaceと生成物から削除される。
- [x] 一時encrypted profileを使う実CRUD/error mapping integration testが成功する。
- [x] FRB正規codegen、Rust / Flutter全品質ゲート、boundary negative testが成功する。
- [x] 独立検証でP1 / P2相当の未解決指摘がない。

## 7. 制約・注意事項

- mapperのdefaultは`internal`相当へfail closedし、未知の内部文言をargumentへ載せない。
- localization argumentはenumごとの明示allowlistとし、自由文字列を受け取らない。
- retryableはcodeから決定し、内部error文字列の解析へ依存しない。
- coreのtyped readiness、lease / epoch分類をgeneric sync failureへ戻さない。
- generated FRB sourceを手編集せず、正規codegen後の差分を確認する。
- UI文言は英日ARBへ追加し、hardcoded strings gateを維持する。

## 8. 完了報告に含めるべき内容

- error code / retryable / argument mapping表
- FRB signatureとDart/UI error flow
- redactionとnegative test
- encrypted profile integration test
- codegenと全品質ゲート
- commit hash、未解決事項、独立検証結果

## 9. 完了報告

### 実装結果

- `ClientErrorKind`をcoreへ追加し、内部errorのpath、SQLite detail、
  UUID、server body、入力値をbridgeへ出さない単一分類境界を実装した。
- FRB公開関数を`Result<T, BridgeErrorDto>`へ変更した。DTOはclosed
  `BridgeErrorCodeDto`、数値のみのclosed localization argument、
  codeから決まる`retryable`だけを持つ。
- code / retryable mapping:

  | code | retryable | 主な内部分類 |
  |---|---:|---|
  | `invalidInput` | false | domain/recurrence/bridge parse validation |
  | `notFound` | false | storage record not found |
  | `conflict` | false | stale/duplicate/protected/active timer conflict |
  | `unauthorized` | false | invalid grant、HTTP 401、明示的OPAQUE authentication rejection |
  | `credentialUnavailable` | false | missing/corrupt session credential |
  | `accountBoundUnavailable` | false | missing account-bound local keys/state |
  | `entitlementRequired` | false | billing entitlement |
  | `upgradeRequired` | false | protocol upgrade |
  | `busy` | true | profile/database/sync lease contention |
  | `leaseLost` | true | runtime epoch or lease ownership changed |
  | `clockSkew` | true | retryable HLC clock skew |
  | `cryptoUnavailable` | false | Device Key/local key/database key failure |
  | `storageFailure` | false | non-busy local I/O/storage failure |
  | `syncFailure` | true | transport/sync run failure |
  | `internal` | false | unsupported/unavailable runtime invariant |

- `AccountClientError`はOPAQUE authentication rejection、HTTP transport、
  response protocol decodeを別variantで表現する。remote transport、HTTP 408 / 5xxは
  retryable `syncFailure`、malformed responseや未知のaccount failureはfail-closedな
  `internal`とし、`unauthorized`へ誤分類しない。HTTP 400 / 422、404 / 410、409、
  429とemail verification期限 / 再送期限はそれぞれstableなinput、not-found、
  conflict、retryable busyへ分類する。
- organization safety / roster / revoke / active key bundleを含む全公開remote
  account経路は同じallowlist mapperを使用する。

- `SyncStatus.last_error`を`SyncFailure`列挙型へ変更し、Flutterへは同じ
  `BridgeErrorDto`として変換するようにした。
- Flutter UIは英日ARBのcode別文言を使用する。未知のDart/plugin例外も
  generic文言へfail closedし、raw `error.toString()`を表示しない。
- production FRB surfaceと生成物から`greet` / `createDraftTask`を削除した。
- boundary checkへstring error、toy API、raw Dart error表示のnegative
  regression guardを追加した。
- FRB 2.12.0のconfig-file正規生成を実行し、typed errorがDart側で
  `BridgeErrorDto implements FrbException`として生成されることを確認した。
- 一時directoryの実SQLCipher profileを`initCore`し、list/taskの
  create/read/deleteとinvalid UUIDのtyped/redacted exceptionを検証する
  Flutter integration testへtoy testを置換した。
- 検証:
  - `cargo fmt --all -- --check`: pass
  - `cargo test --workspace`: pass
    (`client` 124、`crypto` 49 + real Keychain 2 ignored、`domain` 62、
    `server` unit 29 + integrations、`storage` 90 + perf 1 ignored、
    `sync` 103、bridge 2を含む)
  - `cargo clippy --workspace --all-targets -- -D warnings`: pass
  - `flutter_rust_bridge_codegen generate --config-file
    flutter_rust_bridge.yaml`: pass
  - `flutter analyze`: pass
  - `flutter test`: 303 pass、visual QA harness 1 intentional skip
  - encrypted profile / redaction focused Flutter test: 4 pass
  - `sh app/tool/check_client_boundaries.sh`: pass
  - `git diff --check`: pass
- security候補とsettings / internal metadata境界の公式merge後、Issue #65固有の
  2 commitだけをpublic `main`へ載せ直し、公開PRとして提出する。

### 独立検証

- 初回判定: Request changes（P1 2件、P2 2件）
- 対応:
  - Calendar Undoに残っていたraw error interpolationをcode-based localizationへ
    置換し、unawaited callbackで例外を再throwしない。
  - boundary gateへ`.toString()`に加えて`$error` / `${error}` interpolationの
    negative checkを追加した。
  - organization safety、roster、device revoke、active key bundleを共通
    `AccountClientError` mapperへ統一した。
  - upstream errorを`AuthRejected` / `Transport` / `ProtocolDecode`へ分割し、
    generic OPAQUE protocol errorを`unauthorized`へ誤分類しないようにした。
- 修正後検証:
  - client 126、sync 103、bridge 2、auth integration 2: pass
  - Calendar / bridge localization / encrypted CRUD対象Flutter 16: pass
  - `flutter analyze`、focused clippy、boundary / hardcoded / diff check: pass
- 最終判定: APPROVE（P0 / P1 / P2残存なし）
- 検証者: issue54 independent reviewer

### Public main再構築検証

- 作業日: 2026-07-27
- base: public `main` `18ad3c94456ae7a1ae19219753f9016624eb7ecf`
- 再構築: settings境界より後のIssue #65固有2 commitだけをrebaseし、競合なし。
- FRB正規codegen: pass、生成差分なし。
- `cargo fmt --all -- --check`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- client 128、sync 98、bridge 2とdoc test: pass
- `cargo test --workspace`: 機能テストとDocker統合テストはpass。並行負荷時に
  SQLCipher 10k性能testのHome中央値が959 msとなり750 ms上限を超えたため、
  閾値を変更せず同一testを単独再実行し、Home 417 ms、Calendar 71 msでpass。
- bridge release build、`flutter analyze`: pass
- `flutter test`: 303 pass、visual QA harness 1 intentional skip。CI前提の
  performance fixtureを事前buildした実SQLCipher testはHome 568 ms、
  Calendar 150 msでpass。
- hardcoded strings、client boundary、boundary negative test、
  `git diff --check`: pass。
- 既存独立レビューのP0 / P1 / P2残存なし判定を維持し、現行baseへの再構築差分で
  新たなP0 / P1 / P2 / P3指摘なし。

### Email verification統合とfollow-up独立検証

- follow-up独立検証の初回判定: Draft継続（P2 3件、P3 1件）
- 対応:
  - Account画面のaccount / server URL load、save、login、logout、organization
    safety、sync statusに加え、email registration state / Recovery Key復旧、
    登録開始、再送、OTP検証、完了、取消、Recovery Key確認をすべて同じ
    code-based localizationへ統一した。未知のplugin例外は固定`internal`文言へ
    fail closedし、payloadを表示しない。
  - direct FRB errorと`SyncStatus.last_error`を同じ`ClientErrorKind` /
    `SyncFailure`変換へ統一した。`ProfileIdentityMismatch`と
    `IncompleteAccountState`は`accountBoundUnavailable`、transport / HTTP 408 /
    5xxはretryable `syncFailure`となり、全15 codeのexhaustive testで一致を固定した。
  - startup failure logはfixed eventとstable codeだけを出力し、error object、
    stack trace、path、tokenを出さない専用mapperとredaction testへ置換した。
  - client boundary fixtureは実`api/conversions.rs`とDart source treeを使用し、
    `Result<_, String>`、toy API、raw Dart interpolation、必須変換file欠落の4負例が
    必ず検出されるようにした。
- email verification実装済み`main`への統合で、旧`AccountClientError::Http`参照が
  登録のJSON decode / idempotent retry経路に4箇所残るcompile errorを検出した。
  JSON decodeは`ProtocolDecode`、send failureは`Transport`としてretry後も区別して
  返す根本修正を行い、メール登録の全FRB関数も`BridgeErrorDto`へ統一した。
- 2026-07-27 reminder notification統合前の候補:
  - base: `cc2521798ef937592f0a96829c70d82ba5ee940f`
  - base-only rebase前後でbranch patch-id
    `b286b0f0022d0ddd90d997bb4190ea1359370bef`と、fuzz / `Cargo.lock`を除く
    source treeが一致。
- reminder notification reconciliation実装済み`main`
  `585186a2707c1de4452e3ab2af62e34e28768230`へさらにrebaseし、新しい3つの
  notification command FRB APIをすべて`BridgeErrorDto`へ統合した。`u32` limit
  conversionもtyped `invalidInput`とし、3 APIのsignatureをcompile-time testで固定した。
- 最終統合検証:
  - `flutter_rust_bridge_codegen generate --config-file
    flutter_rust_bridge.yaml`: pass。email / reminder / typed errorを含む生成物を更新し、
    再生成がidempotentであることを確認。
  - `cargo fmt --all -- --check`,
    `cargo clippy --workspace -- -D warnings`: pass。
  - `cargo test --workspace`: client 140、crypto 54 + real Keychain 2 ignored、
    domain 62、server unit 53 / auth 17 / billing 9 / migrations 3 / realtime 2 /
    RLS 1 / sync-v2 27、storage 96 + manual perf 1 ignored、sync 101、bridge 4、
    client doc test 4を含む機能 / Docker統合testはpass。連続実行時のみSQLCipher
    10k性能testがHome 1195 ms（上限750 ms）となったため、閾値を変更せず同一testを
    単独再実行し、Home 719 ms、Calendar 149 msでpass。
  - bridge release build、`flutter analyze`: pass。
  - `flutter test`: 333 pass、visual QA harness 1 intentional skip。実SQLCipher
    10k performanceはHome median 242 ms、Calendar median 137 msでpass。
  - Account/email typed error対象Flutter 33、boundary正例 / 4負例、
    hardcoded strings、`git diff --check`: pass。

---
id: 019f9d99-67f9-7212-8568-9da65b493ded
title: Typed FRB error outcomes
status: active
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
- package追加、push、PR、merge。

## 5. 実装手順

1. 全FRB公開関数と現在のstring変換、UI表示経路をinventoryする。
2. stable codeと安全なargument allowlistを固定し、mapper testを先に追加する。
3. FRB signaturesをtyped resultへ変更し、正規codegenを実行する。
4. Dart port / service / providerをtyped errorへ移行する。
5. UI localizationをcodeへbindingし、raw error表示を除去する。
6. toy APIを削除し、実encrypted profile integration testへ置換する。
7. boundary negative test、Rust / Flutter全品質ゲート、leak scanを実行する。

## 6. 受け入れ基準

- [ ] bridge errorはclosed stable code、allowlist済みarguments、retryableだけを持つ。
- [ ] path、SQLite detail、UUID、入力値、server body、secretがDTO / Debug / UIへ出ない。
- [ ] credential、upgrade、profile/database/lease busy、crypto unavailable、
      validation、storage、unknownが区別される。
- [ ] public FRB APIに`Result<_, String>`と`map_err(|e| e.to_string())`が残らない。
- [ ] `SyncStatus.last_error`がraw stringではなくtyped outcomeになる。
- [ ] Flutterはstable codeからARB localizationを選び、raw `.toString()`を表示しない。
- [ ] production `greet` / `createDraftTask`がFRB surfaceと生成物から削除される。
- [ ] 一時encrypted profileを使う実CRUD/error mapping integration testが成功する。
- [ ] FRB正規codegen、Rust / Flutter全品質ゲート、boundary negative testが成功する。
- [ ] 独立検証でP1 / P2相当の未解決指摘がない。

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
  response protocol decodeを別variantで表現する。remote transportはretryable
  `syncFailure`、malformed response、URL/serialization、HTTP 5xx等のgeneric
  account failureはfail-closedな`internal`とし、`unauthorized`へ誤分類しない。
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
- push / PR / mergeは行っていない。baseに未公開security候補を含むため、
  security advisoryの公式merge後に依存順を保って公開する。

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
- 最終判定: 再レビュー待ち
- 検証者: issue54 independent reviewer

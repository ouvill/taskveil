---
id: 019f9aa6-8cc9-7d73-9617-d2a108498ca5
title: Rust storage and sync module decomposition
status: done
lane: standard
milestone: maintenance
---

## 1. 背景とコンテキスト

Rust実装の成長に対してcrate内のmodule分割が追いついていない。特に
`core/storage/src/lib.rs` はrepository、transaction、sync state、local crypto、
reminderとそれらのテストを同居させ、`core/sync/src/apply.rs` は同期run、
full resync、quarantine、collection別pull適用、plaintext変換を同居させている。

crate間の依存境界は維持できているため、本作業では挙動や外部契約を変えず、
module内の責務だけを分割する。schema、wire protocol、暗号形式、FRB APIを変更しない
純粋な構造リファクタなので標準レーンとする。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/task-75-core-extraction-refactor.md`
- `core/storage/src/lib.rs`
- `core/storage/src/migrations.rs`
- `core/sync/src/apply.rs`
- `core/sync/src/enqueue.rs`
- `core/sync/src/lib.rs`

## 3. ゴール

- `taskveil-storage` のpublic APIを維持したまま、型、transaction primitive、
  repository実装、テストを責務別moduleへ分ける。
- `taskveil-sync` のpublic APIを維持したまま、同期オーケストレーションと
  pull適用処理を段階・collection別moduleへ分ける。
- 新しい機能単位を追加するとき、単一の巨大ファイルを横断せず変更箇所を特定できる
  module構成にする。
- 既存テストを削除・skip・弱体化せず、workspace品質ゲートを維持する。

## 4. スコープ

### やること

- `core/storage/src/lib.rs` を責務別のprivate moduleへ分割し、既存public itemを
  crate rootから再exportする。
- storageのinline testを対象責務に近いtest moduleへ移す。
- `core/sync/src/apply.rs` を同期run、resync/pull、record適用、decodeなどの
  cohesiveなsubmoduleへ分割する。
- sync applyのinline testを対象責務に近いtest moduleへ移す。
- module visibilityを必要最小限の`pub(crate)`に調整する。
- 分割後の行数とテスト件数を記録する。

### やらないこと

- public Rust API、error variant、SQL、DB schema、migrationの変更。
- sync wire type、protocol version、collection名、AAD、暗号化blob形式の変更。
- merge、HLC、full resync、quarantine、rotationの意味論変更。
- FRB API、生成物、Flutter/Dart、server実装の変更。
- 新規依存の追加。
- テスト削除、skip化、期待値の弱体化。

## 5. 実装手順

1. storageとsync applyのtop-level item、内部依存、test fixtureを分類する。
2. storageは共通型・transaction・repository群をmoduleへ移し、crate rootを
   module宣言と再export中心にする。
3. sync applyは公開entry pointを維持し、orchestrationとrecord適用の内部処理を
   submoduleへ移す。
4. `cargo fmt`、対象crate test、clippyを実行し、visibilityや循環依存を修正する。
5. 統合HEADでworkspace品質ゲートを実行する。
6. 実装を担当していないエージェントがdiffと統合HEADを独立検証する。

## 6. 受け入れ基準

- [x] `core/storage/src/lib.rs` がmodule宣言、共通定数、再exportを中心とする
      1,000行未満のcrate rootになっている。
- [x] `core/sync/src/apply.rs`、または置換後の`apply/mod.rs`が公開entry pointと
      orchestration中心の1,000行未満のmodule rootになっている。
- [x] 分割対象に1,500行を超える非test source fileを新たに作っていない。
- [x] 既存のpublic import pathが維持され、frontend/client境界checkが成功する。
- [x] SQL、schema version、migration、wire protocol、暗号定数、FRB生成物に
      意図した差分がない。
- [x] `cargo fmt --all -- --check` が成功する。
- [x] `cargo clippy --workspace -- -D warnings` が成功する。
- [x] `cargo test --workspace` が成功する。
- [x] `sh app/tool/check_client_boundaries.sh` が成功する。
- [x] `sh app/tool/test_client_boundaries.sh` が成功する。
- [x] `git diff --check` が成功する。
- [x] 独立検証者が統合diffと品質ゲートを確認し、結果を完了報告へ記録している。

## 7. 制約・注意事項

- 移動とvisibility調整以外の意味論変更を同じ差分へ混ぜない。
- `pub(crate)`はsubmodule間で本当に必要なitemだけに限定する。
- test helperの共有を理由にproduction visibilityを広げない。
- storageとsyncのcrate境界、`LocalSyncStore` trait注入、frontendから
  `taskveil-client`への一方向依存を維持する。
- 同期・暗号・DBの仕様変更が必要と判明した場合は実装を止め、重要変更レーンの
  別work itemとして扱う。
- public repoへprivate情報を記録しない。

## 8. 完了報告に含めるべき内容

- 分割前後のファイル別行数。
- 旧責務から新moduleへの対応。
- public API、schema、wire format、暗号形式、FRB APIが不変である根拠。
- 実行した品質ゲートとtest結果。
- 独立検証の判定、根拠、検証者。
- 未解決事項。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果:
  - `core/storage/src/lib.rs` をcrate rootのmodule宣言とpublic re-export中心へ変更し、
    model、error、trait、transaction、database、row mapping、各repository、
    full resync、責務別test moduleへ分割した。
  - `core/sync/src/apply.rs` を公開entry pointと共通型中心へ変更し、
    orchestration、resync、pull、collection別record適用、decode、test moduleへ
    分割した。
  - 新moduleはexternal private、兄弟module間共有は`pub(super)`に限定し、
    既存のcrate-root APIと`taskveil_sync::apply::*` APIを維持した。
- 行数:

  | 対象 | before | after |
  |---|---:|---:|
  | `core/storage/src/lib.rs` | 10,554 | 82 |
  | `core/sync/src/apply.rs` | 4,120 | 88 |
  | storage新規production最大 | - | 891 (`sync_state_repository.rs`) |
  | sync apply新規production最大 | - | 1,021 (`apply/records.rs`) |

- 主な分割対応:

  | 旧配置 | 新配置 |
  |---|---|
  | storage共通型・error・trait | `models.rs` / `error.rs` / `traits.rs` |
  | storage DB open・transaction・row変換 | `database.rs` / `transaction.rs` / `row.rs` |
  | storage domain repository | `*_repository.rs` |
  | storage sync state・full resync | `sync_state_repository.rs` / `full_resync.rs` |
  | storage inline tests | `tests/` 配下の4責務群と共通fixture |
  | sync run・preflight・canonical Inbox | `apply/orchestration.rs` |
  | full resync | `apply/resync.rs` |
  | delta page・quarantine・push応答整合 | `apply/pull.rs` |
  | collection別pull適用 | `apply/records.rs` |
  | envelope/HLC/plaintext decode | `apply/decode.rs` |
  | sync apply inline tests | `apply/tests.rs` |

- 契約不変の証拠:
  - baseline/currentのrustdocを同一toolchainで比較し、storage crate rootとsync
    apply rootのpublic item、宣言、public/trait method signatureの追加・欠落・変更は0。
  - brace-aware item比較でstorage 556/556、sync apply 119/119のfunction bodyが、
    visibility・空白・trailing commaを除いて完全一致した。
  - `migrations.rs`、`schema.sql`、sync `protocol.rs`、`envelope.rs`、`merge.rs`の
    SHA-256がbaselineと一致した。
  - source test名はstorage 90/90、sync 89/89で一致し、ignoreは既存のstorage
    performance test 1件だけである。
- 検証:
  - `cargo fmt --all -- --check`: 成功。
  - `cargo clippy --workspace -- -D warnings`: 成功。
  - `cargo test --workspace`: sandbox内の初回実行ではclient testのlocal socket
    bindが`PermissionDenied`となった。コード起因ではないため同一HEADをsandbox外で
    再実行し、全workspace testが成功した。
  - `cargo test -p taskveil-storage -p taskveil-sync`: storage 89成功・既存1 ignore、
    sync 89成功。
  - `sh app/tool/check_client_boundaries.sh`: 成功。
  - `sh app/tool/test_client_boundaries.sh`: 成功。
  - tracked/untracked双方のwhitespace check: 成功。
- Commit: 未コミット
- 未解決: なし

### 独立検証

- 判定: 合格
- 根拠:
  - baseline `2f6c7c99fe9413fa7e517833fb4ac91767582e8b` と統合差分を比較し、
    許可範囲外、public API、function body、SQL/schema/migration、sync wire/暗号、
    test名、module visibility、client boundaryに意図しない変更がないことを確認した。
  - `cargo fmt --all -- --check`、対象crate test、client boundary scripts、
    hash/diff/whitespace checkを独立再実行して成功した。
- 検証者: `contract_audit` サブエージェント（実装非担当）

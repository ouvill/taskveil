---
id: 019f9a15-78b3-7372-a22f-36d4e38c9cb1
title: SQLite sync adapter parity
status: active
lane: standard
milestone: maintenance
---

# SQLite sync adapter parity

## 1. 背景とコンテキスト

`core/client/src/sqlite_sync_store.rs` は、通常のconnection-per-operation adapterである
`SqliteSyncStore` と、atomic sync run用の `SqliteSyncWriteTx` を実装している。両者は
`LocalMutationSyncStore` / `LocalSyncStore` の同じ44 methodを別々に実装しており、
storage型とsync型の変換にもinline実装と共通helperの混在がある。

direct adapterとtransaction adapterではDB open、lock、commit / rollback semanticsが
意図的に異なる。一方、永続化されるoutbox、record state、cursor、quarantine、alias、
domain recordの表現と読み取り結果は一致する必要がある。責務分割前にこのparityを
contract testで固定し、低リスクな変換境界だけをmoduleへ抽出する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `core/client/src/sqlite_sync_store.rs`
- `core/sync/src/enqueue.rs`
- `core/storage/src/lib.rs`

## 3. ゴール

- direct / transaction adapterが全代表カテゴリで同じ永続化結果を返すことを検証する。
- transactionのcommitで全カテゴリが永続化され、dropで全カテゴリがrollbackされることを
  検証する。
- storage型とsync型のpure conversionを子moduleへ集約し、adapter本体から表現変換責務を
  分離する。
- public API、DB schema、sync protocol、lock、open、transaction semanticsを変更しない。

## 4. スコープ

### やること

- outbox live / tombstone、record state、cursor、quarantine、list alias、task、list、
  template、task series、timer sessionを含むadapter parity testを追加する。
- transaction commit / rollback contract testを追加する。
- pure conversion helperを `core/client/src/sqlite_sync_store/convert.rs` へ移動する。
- 関連するRust品質ゲートとclient boundary checkを実行する。

### やらないこと

- direct adapterをtransaction adapterへ委譲する変更。
- connection open回数、busy lock、transaction開始方式、commit / rollback境界の変更。
- storage trait、sync trait、public API、schema、protocol、暗号形式の変更。
- direct / transaction adapter実装全体の抽象化または大規模module分割。
- 新規依存追加。

## 5. 実装手順

1. 代表カテゴリを同一fixtureで書込み・読取りするgeneric contract helperをtestへ追加する。
2. direct adapterとtransaction commit後のsnapshotが一致することを検証する。
3. transaction drop後に全代表カテゴリが残らないことを検証する。
4. contract test合格後、pure conversion helperを子moduleへ内容を変えずに移動する。
5. format、clippy、client test、workspace check、boundary check、diff checkを実行する。
6. 実装結果を完了報告へ記録し、Conventional Commitを作成する。

## 6. 受け入れ基準

- [x] direct / transaction adapterが代表カテゴリで同じsnapshotを返す。
- [x] outboxとquarantineはlive / tombstoneの両方を検証する。
- [x] transaction commit後は全代表カテゴリが永続化される。
- [x] transaction drop後は全代表カテゴリがrollbackされる。
- [x] pure conversion helperが子moduleに集約されている。
- [x] direct / transactionのopen、lock、commit semanticsに変更がない。
- [x] public API、schema、protocol、依存に変更がない。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy -p taskveil-client --all-targets -- -D warnings`
- [x] `cargo test -p taskveil-client`
- [x] `cargo check --workspace --all-targets`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`

## 7. 制約・注意事項

- direct adapterはoperationごとにencrypted DBをopenし、transaction adapterは
  `OwnedSqliteWriteTx` を保持する。この差を共通化しない。
- `StorageError::NotFound` から `Option::None` へのmapping、collection検証、
  live / tombstone tagを維持する。
- transaction testはcommitとdropを別DBで確認し、途中状態を外部connectionから読まない。
- 秘密鍵、復号済みplaintext、tokenをログや完了報告へ含めない。
- 独立検証前はstatusを `active` に保つ。

## 8. 完了報告に含めるべき内容

- 追加したcontract testと検証カテゴリ。
- 抽出したconversion helperと、変更しなかったadapter semantics。
- 実行した品質ゲートの結果。
- Commit hash。
- skip、環境制約、未解決事項。

## 9. 実装結果

実装日: 2026-07-26

- `AdapterFixtures` とgenericなseed / snapshot helperを追加し、direct adapterと
  transaction commit後の結果が一致することを固定した。
- setting、outbox live / tombstone、record state live / tombstone、cursor、
  quarantine live / tombstone、list alias、list、task、template、task series、
  timer sessionを同一fixtureで検証した。
- transactionをcommitせずdropした後、同じ全カテゴリが残らないことを検証した。
- storage型とsync型のoutbox、record state、quarantine、full resync、cursor、
  sweep summary、list alias変換を `sqlite_sync_store/convert.rs` へ移した。
  direct / transaction adapterは同じoutbox変換を共有する。
- connection-per-operation、encrypted DB open、`OwnedSqliteWriteTx` の保持、
  alias materialization transaction、commit / drop rollback境界は変更していない。
- public API、DB schema、sync protocol、Cargo manifest、依存追加は変更していない。

品質ゲート:

- `cargo fmt --all -- --check`: pass
- `cargo clippy -p taskveil-client --all-targets -- -D warnings`: pass
- `cargo test -p taskveil-client`: pass（unit 59、doc-test 4）
- `cargo check --workspace --all-targets`: pass
- `sh app/tool/check_client_boundaries.sh`: pass
- `sh app/tool/test_client_boundaries.sh`: pass
- `git diff --check`: pass

環境メモ:

- `cargo test -p taskveil-client` のsandbox内初回実行では、既存のlocal HTTP server
  testがsocket bindを拒否された。許可済みのsandbox外再実行では全件passした。
- skip、未解決事項なし。
- Commit: この変更を含むcommit（最終hashはGit履歴を正本とする）

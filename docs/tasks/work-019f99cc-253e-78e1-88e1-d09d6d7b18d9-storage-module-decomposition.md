---
id: 019f99cc-253e-78e1-88e1-d09d6d7b18d9
title: Storage migration module decomposition
status: done
lane: standard
milestone: maintenance
---

# Storage migration module decomposition

## 1. 背景とコンテキスト

`core/storage/src/lib.rs` はlocal SQLCipher DBの公開contract、repository実装、transaction、
schema migration、約90件のunit testを単一ファイルに保持し、11,000行を超えている。
とくにschema migrationはv2からv22までの履歴と長いSQLを含む一方、通常のrepository変更と
同じファイルで競合し、変更レビュー時にschema不変性を確認しにくい。

migration集合は `open_encrypted` から呼ばれる `ensure_schema` と、canonical Inbox
materializationから参照される `read_user_version` を除いて内部で閉じている。公開APIや
schemaを変えずにこの集合だけをmoduleへ移し、後続の保守性改善に先立つ安全な境界を作る。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md` §5
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `core/storage/src/lib.rs`
- `core/storage/src/schema.sql`
- `docs/adr/ADR-017.md`
- `docs/adr/ADR-018.md`
- `docs/adr/ADR-021.md`
- `docs/adr/ADR-023.md`
- `docs/adr/ADR-024.md`

## 3. ゴール

- schema migrationの定義、runner、各versionの適用関数、baseline検証を
  `core/storage/src/migrations.rs` に集約する。
- `core/storage/src/lib.rs` の責務と行数を減らし、migration履歴を単独でレビュー可能にする。
- public API、local DB schema、migration動作、依存関係を変更しない。

## 4. スコープ

### やること

- `SCHEMA`、`BASELINE_SCHEMA_VERSION`、`MIGRATIONS`、`Migration` を
  `core/storage/src/migrations.rs` へ内容を変えずに移動する。
- `ensure_schema` から `table_columns` までのmigration集合を同moduleへ内容を変えずに移動する。
- `lib.rs` から必要な内部itemを参照できる最小限のmodule visibilityとimportを設定する。
- 対象品質ゲートを実行し、結果を完了報告へ記録する。

### やらないこと

- `LATEST_SCHEMA_VERSION`、`StorageError`、`open_encrypted`、
  `rekey_encrypted_database`、`apply_sqlcipher_key`、公開型・trait・repository型の移動。
- `schema.sql`、SQL文字列、migration順序・名前・function pointerの変更。
- migrationやrepositoryの振る舞い、public API、crate依存の変更。
- test moduleや他repository実装の分割。
- rename、整形、リファクタリング、新規依存追加。

## 5. 実装手順

1. `lib.rs` のmigration関連itemと、そのproduction/test参照を再確認する。
2. flat fileの `core/storage/src/migrations.rs` を追加し、指定された集合を純粋移動する。
3. `lib.rs` にprivate module宣言と必要な内部importだけを追加する。
4. format、storage clippy/test、workspace check、client boundary、diff検証を実行する。
5. 実装結果と検証事実を `## 9. 完了報告` に追記する。

## 6. 受け入れ基準

- [ ] `core/storage/src/migrations.rs` に指定されたmigration集合が集約されている。
- [ ] `core/storage/src/schema.sql` に差分がない。
- [ ] `LATEST_SCHEMA_VERSION` は22のままrootで公開されている。
- [ ] migration targetは2から22まで連続し、順序・名前・function pointerが変更されていない。
- [ ] `StorageError`、`open_encrypted`、`rekey_encrypted_database`、
      `apply_sqlcipher_key`、全公開型・traitがrootに残っている。
- [ ] SQL文字列とmigration処理の内容が変更されていない。
- [ ] crate依存が追加・変更されていない。
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy -p taskveil-storage --all-targets -- -D warnings`
- [ ] `cargo test -p taskveil-storage`
- [ ] `cargo check --workspace --all-targets`
- [ ] `sh app/tool/check_client_boundaries.sh`
- [ ] `sh app/tool/test_client_boundaries.sh`
- [ ] `git diff --check`

## 7. 制約・注意事項

- public repoの保守作業であり、private repoの情報を記録しない。
- flat `src/migrations.rs` として `include_str!("schema.sql")` の相対pathを維持する。
- inline testはmigration内部itemを直接参照するため、crate外へ公開せずparent moduleからだけ
  参照できるvisibilityにする。
- `read_user_version` はtestだけでなくcanonical Inbox materializationからも使われる。
- 長いSQLを伴うため、純粋移動と必要なimport・visibility以外の差分を混ぜない。
- 実装者は独立検証の合否を記入せず、front matterを `done` にしない。

## 8. 完了報告に含めるべき内容

- 移動したitemの範囲と、rootに残したpublic contract。
- `lib.rs` と追加moduleの行数。
- schema、migration順序、SQL、public API、依存が不変である確認結果。
- 実行した品質ゲートのコマンドと成否。
- skip、環境制約、未解決事項がある場合の再現可能な詳細。
- Commitは独立検証前の未コミット状態なら「未コミット」と記録する。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: schema migrationの定義、runner、v2からv22の適用関数、baseline検証を
  `core/storage/src/migrations.rs` へ移動した。`core/storage/src/lib.rs` は11,851行から
  10,554行へ減り、追加した `migrations.rs` は1,318行となった。
- 不変条件: `schema.sql`、Cargo依存、public API、migration v2からv22の順序・名前を
  変更していない。`LATEST_SCHEMA_VERSION`、`StorageError`、open / rekey / key適用、
  公開型・traitはcrate rootに残した。
- 修正経緯: 初回test compileでinline testが直接参照するinternal migration helperの
  visibility漏れを検出し、crate外へ公開しない `pub(super)` とtest専用importで修正した。
- 証拠: `cargo fmt --all -- --check`、storage対象clippy、storage test
  （89 passed、manual performance test 1 ignored）、workspace all-target check、
  client boundary check 2種、`git diff --check` が成功した。
- Commit: 未コミット
- 未解決: なし

### 独立検証

- 判定: 合格
- 構造レビュー: `HEAD` の移動元と `migrations.rs` を正規化比較し、差分が
  parent moduleからの参照に必要な `pub(super)`、空行、rustfmtによる2関数の改行だけで
  あることを確認した。migration target 2から22の順序、名前、function pointer、
  SQL文字列と処理内容は不変だった。
- 契約レビュー: `schema.sql`、root / storage `Cargo.toml`、`Cargo.lock` は
  `HEAD` とSHA-256が一致した。crate rootの公開宣言も機械比較で一致し、
  `LATEST_SCHEMA_VERSION = 22`、`StorageError`、open / rekey / key適用、
  全公開型・traitがrootに残っている。追加moduleはprivateで、parent testとproduction
  呼び出しに必要なitemだけが `pub(super)` だった。
- 再実行: `cargo fmt --all -- --check`、
  `cargo clippy -p taskveil-storage --all-targets -- -D warnings`、
  `cargo test -p taskveil-storage`（89 passed、1 ignored）、
  `cargo check --workspace --all-targets`、
  `sh app/tool/check_client_boundaries.sh`、
  `sh app/tool/test_client_boundaries.sh`、`git diff --check` はすべて成功した。
- 検証者: 実装に関与していない独立検証エージェント

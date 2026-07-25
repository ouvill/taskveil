---
id: 019f9b4e-42aa-7572-ab13-571293272f56
title: Client SQL migration ledger
status: done
lane: critical
milestone: maintenance
---

# Client SQL migration ledger

## 1. 背景とコンテキスト

Taskveilのlocal SQLCipher DBは、`PRAGMA user_version`とRust callbackの配列でv1からv22までを管理している。server側ではSQLx `Migrator`により、不変のSQL file、version、checksum、適用履歴を正本とする運用へ移行した。

一般配布前でlocal DBの後方互換性を要求しない現在、client側も現行schemaを新しいv1へsquashし、`rusqlite` / SQLCipher接続を維持したままSQLx相当の前方migration規律へ統一する。

本変更はlocal DB schema管理方式と開発profileの破棄を伴う重要変更であり、2026-07-26にプロダクトオーナーが本work itemへの着手を承認した。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-011.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `core/storage/src/database.rs`
- `core/storage/src/migrations.rs`
- `core/storage/src/schema.sql`

## 3. ゴール

- 現行v22 schemaを新しいv1 initial migrationへsquashする。
- migration SQLをversion付きfileとしてbinaryへ埋め込む。
- SQLCipher DB内のledgerへversion、name、checksum、適用時刻を記録する。
- 適用済みmigrationの欠落、順序不整合、checksum不一致、新しい未知versionを拒否する。
- migration SQLとledger更新を同じ排他transactionで確定する。
- server/client双方で「適用済みmigrationは変更せず、修正は新しい前方migrationへ追加する」規律を共有する。

## 4. スコープ

### やること

- `core/storage`のmigration runner、initial schema、errorを更新する。
- 旧v1〜v22 callbackとlegacy upgrade fixtureを削除する。
- fresh DB、再open、checksum mismatch、欠落、未知version、rollback、排他を検証する。
- local DB schema管理のADRと技術仕様を更新する。

### やらないこと

- SQLx SQLite driverへの移行。
- `rusqlite` repositoryのasync化または置換。
- 配布済みclient DBのlegacy bridge migration。
- server migration方式の変更。
- Flutter UI、FRB API、sync protocol、暗号suiteの変更。

## 5. 実装手順

1. 現行v22 DBから最終schemaを抽出し、`0001_initial.sql`として固定する。
2. migration manifestとchecksum計算、ledger検証、排他適用を実装する。
3. `open_encrypted`でSQLCipher key設定後に新runnerを呼ぶ。
4. repositoryから旧schema conditionalを除去する。
5. legacy migration testを新契約のtestへ置換する。
6. ADR、技術仕様、開発規約へ運用ルールを反映する。
7. storage testとworkspace品質ゲートを実行する。

## 6. 受け入れ基準

- [x] 空のSQLCipher DBを開くと新v1 schemaとledgerが同一transactionで作成される。
- [x] 再openではmigrationが再適用されない。
- [x] ledgerのchecksum改変、migration欠落、未知versionを明示的に拒否する。
- [x] migration失敗時にschema変更とledger rowが残らない。
- [x] 同一DBへの競合migration開始が直列化される。
- [x] wrong keyをmigration errorではなく既存どおりinvalid keyとして拒否する。
- [x] 現行repository、同期、暗号cache、timer、template / seriesのstorage testが通る。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace -- -D warnings`
- [x] `cargo test --workspace`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`

## 7. 制約・注意事項

- migration ledgerをSQLCipher key設定前に読まない。
- checksum値、migration SQL、暗号鍵をログへ出さない。
- `PRAGMA user_version`とledgerを恒久的な二重正本にしない。
- migration lock取得後にledgerを再読し、複数processのTOCTOUを避ける。
- 適用済みmigration fileは変更しない。
- fresh DBとupgrade DBの最終schemaが分岐するbaseline方式を採用しない。

## 8. 完了報告に含めるべき内容

- 採用したledger schemaとtransaction境界。
- 旧v1〜v22をsquashした結果。
- 追加・置換したmigration test。
- 実行した品質ゲートと環境制約。
- 独立検証の判定と根拠。

## 9. 完了報告

- 旧runnerを空のSQLCipher DBへ実際にv22まで適用し、`sqlite_schema`から最終schemaを抽出した。`0001_initial.sql`はその結果を基礎に、FTS5 shadow tableを直接記述せず、同値な最終DDLへ整形したものであり、推測だけで再作成したschemaではない。
- 旧v22実DBと新v1実DBを別々に生成し、migration ledgerとFTS5 shadow tableを比較対象から除外したうえで、schema object、`table_xinfo`、`index_list` / `index_info`、`foreign_key_list`を正規化比較した。結果は42 object、20 application tableで一致した。
- `cargo test -p taskveil-storage`: 82 passed、0 failed、1 ignored（手動性能test）。
- `cargo test --workspace`: sandbox内の初回実行はclient testのlocalhost bindを拒否したため58 passed後に1 failed。権限付き再実行ではworkspace全体が0 failedで完走した。
- `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、client boundary check / test、`git diff --check`はすべて成功した。
- 独立検証は初回に、手動正規化したtyped due / series provenanceの`CHECK`式を直接検証するtestが不足しているとしてP2不合格となった。fresh DBへ不整合なtyped dueと一部だけ設定したseries provenanceをraw INSERTし、双方の`ConstraintViolation`を確認する回帰testを追加した。
- 同じ独立検証者による再検証では、対象test 1 passed、storage全体82 passed / 1 ignored、storage Clippy、Rustfmt、`git diff --check`、client boundary check / testが成功し、新たな指摘なしでcritical lane合格となった。

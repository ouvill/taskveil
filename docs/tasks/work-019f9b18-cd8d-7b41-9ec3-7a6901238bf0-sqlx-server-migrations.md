---
id: 019f9b18-cd8d-7b41-9ec3-7a6901238bf0
title: SQLx server migration ledger
status: done
lane: critical
milestone: maintenance
---

# SQLx server migration ledger

## 1. 背景とコンテキスト

Postgres migrationのproduction entrypointは、`server/src/db.rs`へ全SQLを手動列挙し、
`taskveil-migrate`の実行ごとに全migrationを再適用している。個々のSQLは原則として
再適用可能だが、適用済みversionとchecksumの共通ledgerがなく、過去SQLの意図しない変更、
列挙漏れ、不要なDDL lockをrunner自身では防げない。

一方、SQLx metadata生成では`cargo sqlx migrate run`を使用しており、production、
local development、SQLx prepareでmigration管理方式が一致していない。

本作業は2026-07-26にプロダクトオーナーから実装修正の承認を得て着手する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-003.md`
- `docs/adr/ADR-008.md`
- `docs/adr/ADR-022.md`
- `docs/ops/runbook-db-migration.md`
- `docs/ops/runbook-server-deploy.md`
- `server/src/db.rs`
- `server/src/bin/taskveil-migrate.rs`
- `tool/dev_server.sh`
- `tool/sqlx_prepare.sh`

## 3. ゴール

- server migrationの正本を`server/migrations/`とSQLx `Migrator`へ統一する。
- `_sqlx_migrations`に適用version、checksum、成功状態を記録し、未適用分だけ実行する。
- 適用済みSQLの変更、dirty migration、欠落migrationをdeploy前に拒否する。
- production、local development、SQLx prepareが同じSQLx migration契約を使う。
- migration専用owner credentialと通常runtime credentialの分離を維持する。

## 4. スコープ

### やること

- `taskveil-server`でSQLx migration featureとcompile-time `Migrator`を有効にする。
- migration directory変更時にserver binaryが確実に再buildされるようにする。
- `run_migrations`の手動SQL列挙と全件再実行をSQLx `Migrator::run`へ置換する。
- `tool/dev_server.sh`を`taskveil-migrate` entrypoint利用へ統一する。
- clean DB、適用済みDB、再実行、checksum不一致をPostgres integration testで検証する。
- 技術仕様とDB migration runbookを実装に合わせて更新する。

### やらないこと

- local SQLCipher migration方式の変更。
- 既存Postgres schema、RLS、同期protocol、暗号形式の変更。
- down migrationまたは自動DB rollbackの導入。
- runtime serverへのowner credential付与。
- production deploy workflowまたはcloud resourceの変更。

## 5. 実装手順

1. SQLx `migrate` featureとembedded migratorを追加する。
2. `run_migrations`をledger検証とpending migration適用だけを行う実装へ置換する。
3. 既存DBは、現行SQLが再適用可能であることを統合テストしたうえで、初回だけ全SQLを
   SQLx経由で再適用してledgerをbootstrapする。
4. local developmentをproductionと同じbinary entrypointへ切り替える。
5. migration ledgerの件数、再実行時にDDLが変化しないこと、checksum不一致拒否、
   旧runner適用済み相当DBからのbootstrapを検証する。
6. runbookと技術仕様を更新し、品質ゲートと独立検証を行う。

## 6. 受け入れ基準

- [x] `run_migrations`にmigration filenameの手動列挙がない。
- [x] clean DBで全migrationが適用され、`_sqlx_migrations`へ全versionが記録される。
- [x] 2回目の実行では適用済みmigrationのSQLが再実行されない。
- [x] 適用済みmigrationのchecksum不一致を明示的に拒否する。
- [x] 旧runnerで全SQL適用済みかつledgerがないDBを一度だけ安全にbootstrapできる。
- [x] migration directoryへのSQL追加がserver binaryの再build対象になる。
- [x] local development、SQLx prepare、stagingが同じmigration directoryとSQLx ledgerを使う。
- [x] migration ownerとruntime non-ownerのcredential境界、RLS検証、deploy順が不変である。
- [x] server integration test、workspace品質ゲート、`git diff --check`が成功する。
- [x] 独立検証でP1 / P2相当の未解決指摘がない。

## 7. 制約・注意事項

- 既存migration fileはchecksum契約になるため、導入後は原則変更せず追加migrationで修正する。
- SQLxのPostgres migration lockはdirect owner connectionだけで使用し、transaction poolの
  runtime connectionへ持ち込まない。
- 旧DBのledger bootstrapは現行SQLの再適用耐性に依存するため、統合テストで明示的に固定する。
- migration失敗時はLambda aliasとWorkerを切り替えず、DBはforward fixを原則とする。
- public repoへ実connection string、secret、private deployment inventoryを記録しない。

## 8. 完了報告に含めるべき内容

- SQLx ledgerへ統一したentrypointと旧DB bootstrap方式。
- 再実行されないこととchecksum不一致拒否の観測可能なテスト結果。
- 実行したserver / workspace品質ゲート。
- 独立検証の判定、根拠、検証者。
- 未解決事項または後続作業。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: server migration runnerを手動列挙と全SQL再適用からcompile-time SQLx
  `Migrator`へ置換した。`_sqlx_migrations`のversion / checksum検証、pending migration
  だけの適用、migration directory変更時の再build、local developmentの同一binary利用を
  実装した。
- 旧DB移行: ledger導入前の12 migration適用済みDBは初回だけSQLx経由で再適用し、
  13本目の保護migrationを含むledgerを作成する。user、device、tenant、key、sync task、
  billing、session、access / refresh tokenと一度限りのreset markerが保持されることを
  Postgres integration testで確認した。
- 権限境界: `_sqlx_migrations`と`taskveil_schema_migrations`から`taskveil_app`の全権限を
  REVOKEした。clean / legacy bootstrapの両方で、実runtime LOGIN roleが両tableの
  SELECT / INSERT / UPDATE / DELETE権限を持たないことを確認した。
- 証拠: `migrator_records_versions_skips_applied_sql_and_rejects_checksum_changes`と
  `migrator_bootstraps_ledger_for_database_created_by_legacy_runner`が成功した。2回目の
  migration実行前後でcollection constraint OIDが不変であり、checksum改変は
  `VersionMismatch`として拒否された。
- 品質ゲート: `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、
  `cargo test --workspace`、client boundary check / test、`bash -n tool/dev_server.sh`、
  `git diff --check`が成功した。Flutter変更はないためFlutter gateは対象外とした。
- Commit: 未コミット
- 未解決: なし

### 独立検証

- 判定: 合格
- 根拠: 初回レビューではmigration ledgerへのruntime DML権限、旧DBデータ保持テスト不足、
  運用ガイドの旧psql記述が指摘された。修正後の再検証で、owner-only ledger、
  clean / legacy両経路のruntime権限拒否、代表データ保持、文書整合を確認した。
  `cargo test -p taskveil-server --test migrations`は2件成功し、`git diff --check`も成功した。
  未解決のP1 / P2指摘はない。
- 検証者: 独立エージェント `/root/verify_sqlx_migrations`

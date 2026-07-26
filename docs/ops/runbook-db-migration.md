# DBマイグレーションrunbook

TaskveilサーバーDBのマイグレーション手順を定義する。2026-07-08時点の本番デプロイは未実施であり、このrunbookはローカルリハーサルと将来のNeon適用に向けたドラフトである。

## 1. 対象

対象は [`server/migrations/`](../../server/migrations/) 配下のPostgres SQLである。`taskveil-migrate` binaryだけが`DATABASE_MIGRATION_URL`のowner接続でSQLx `Migrator`を実行する。SQLxは`_sqlx_migrations`へversion、description、適用日時、成功状態、checksum、実行時間を記録し、適用済みchecksumを検証して未適用migrationだけを順に実行する。`_sqlx_migrations`と一度限りのreset marker `taskveil_schema_migrations`はowner-onlyとし、`taskveil_app`へ権限を付与しない。通常の`taskveil-server`は起動時migrationを行わず、owner credentialを受け取らない。ローカル開発でも [`tool/dev_server.sh`](../../tool/dev_server.sh) が同じ`taskveil-migrate` binaryを使用する。

## 2. 方針

- マイグレーションは前方のみとする。
- ロールバックはDBを巻き戻すのではなく、修正SQLまたは修正版アプリで前方に進める。
- 適用済みmigration fileは変更しない。SQLxのchecksum不一致は失敗として扱い、修正は新しいmigrationへ追加する。
- 互換性が必要な変更はexpand-contractを使う。
- 既存データ削除、列の意味変更、型変更、NOT NULL追加、unique制約追加は、リハーサルと影響確認なしに行わない。
- E2EEデータの暗号blobをサーバー側で復号する作業は行わない。

## 3. expand-contractの目安

1. expand: nullable列、別名列、新テーブル、互換indexなどを追加する。
2. app update: 新旧schemaを両方扱えるサーバー/クライアントを出す。
3. backfill: 必要なデータを埋める。大量更新はバッチ化する。
4. contract: 全クライアント/サーバーが新schema前提になってから旧列・旧テーブルを撤去する。

contractは公開リポジトリだけで判断しない。実稼働状況、クライアント普及率、保持期間はprivate側/人間管理で判断する。

## 4. ローカルリハーサル

既存の開発DBを使う場合:

```sh
./tool/dev_server.sh
```

スクリプトは `taskveil-dev-postgres` を起動し、`taskveil-migrate`で未適用migrationを適用してからサーバーを起動する。

クリーンDBで試す場合:

```sh
docker rm -f taskveil-dev-postgres
./tool/dev_server.sh
```

ヘルスチェック:

```sh
curl -i http://localhost:8080/health
```

2台同期の回帰確認:

```sh
# 詳細は docs/dev/two-device-sync-test.md
```

## 5. SQL追加時の確認項目

- ファイル名は既存の連番形式に合わせる。例: `YYYYMMDDNNNN_description.sql`
- 既存環境へ一度だけ適用される前提でSQLを書く。deploy retryとtransaction rollback後の再実行も考慮する。
- 適用済みmigration fileは変更せず、新しい連番fileを追加する。
- `server/src/db.rs`へのfilename列挙は不要である。`server/build.rs`がmigration directoryの変更を検知し、compile-time `Migrator`へ埋め込む。
- `server/tests/` にmigration後の基本CRUDまたはAPIテストを追加する。
- `tool/dev_server.sh` でローカル適用できることを確認する。

## 6. Neon適用手順

実Neonのdirect owner connection stringは `<NEON_MIGRATION_DATABASE_URL>` として扱い、private側または人間管理に置く。public repo、public issue、完了報告、CIログに実値を書かない。

事前にローカルリハーサルを通す。次に、Neonのbranch機能が利用できる場合は本番branchから検証branchを作成し、同じSQLを適用して確認する。

```sh
DATABASE_MIGRATION_URL="<NEON_MIGRATION_DATABASE_URL>" \
  cargo run -p taskveil-server --bin taskveil-migrate
```

deployはLambda alias切替よりmigrationを先に行い、失敗時はaliasを動かさない。SQLは再実行で壊れない設計にする。通常query用の `DATABASE_URL` は別のruntime loginを使用し、migrationが作成するNOLOGIN group role `taskveil_app`のmemberにする。

SQLx ledger導入前に全SQLを適用済みのDBでは、最初の`taskveil-migrate`だけが現行SQLをSQLx経由で再適用して`_sqlx_migrations`を作成する。現行migration集合はこのbootstrap再適用をintegration testで検証する。以後はledgerにないmigrationだけが実行される。`202607240002_task_series_domain.sql`の独自`taskveil_schema_migrations` markerは、zero-knowledge serverで変換できない旧recordの削除をbootstrap時に繰り返さないため維持する。

```sql
-- role名とpasswordは運用環境で管理する。実値をpublic repoへ記録しない。
CREATE ROLE <RUNTIME_LOGIN> LOGIN PASSWORD '<SECRET>'
    NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
GRANT taskveil_app TO <RUNTIME_LOGIN>;
```

本番・共有環境では次を分離する。

- `DATABASE_MIGRATION_URL`: schema owner / migration専用。通常server requestへ渡さない。
- `DATABASE_URL`: pooled endpointを使うnon-owner runtime login。`INHERIT`付きで`taskveil_app`のmemberにし、serverは接続時にLOGIN / non-owner / NOSUPERUSER / NOBYPASSRLS / 権限継承を検証する。transaction poolで保持されないsession-level `SET ROLE`には依存しない。

ローカルの `tool/dev_server.sh` もowner接続と`taskveil_runtime` loginを分離する。

### 6.1 continuity retention migrationの事前確認

`202607260004_continuity_retention.sql`は既存rowをdeviceごとにcompactし、
`continuity_closure_proofs`と`device_resync_sessions`へtenant / device単位の
unique indexを追加する。通常のSQLx migration transactionでは
`lock_timeout = 5s`を設定し、schema lockを無期限に待たない。

適用前にownerのread-only sessionで次を採取し、同程度のrow数を持つ検証branchで
所要時間をリハーサルする。個別のtenant / device IDは記録せず集計値だけを残す。

```sql
SELECT
  'continuity_closure_proofs' AS relation,
  count(*) AS rows,
  count(DISTINCT (tenant_id, device_id)) AS devices,
  count(*) - count(DISTINCT (tenant_id, device_id)) AS rows_to_compact,
  pg_total_relation_size('continuity_closure_proofs') AS total_bytes
FROM continuity_closure_proofs
UNION ALL
SELECT
  'device_resync_sessions',
  count(*),
  count(DISTINCT (tenant_id, device_id)),
  count(*) - count(DISTINCT (tenant_id, device_id)),
  pg_total_relation_size('device_resync_sessions')
FROM device_resync_sessions;
```

release担当は適用前に、同期writeを停止できる時間とmigrationのlock取得許容時間を
明示する。標準migrationのlock取得budgetは5秒である。検証branchでindex buildが
maintenance window内に完了しない、または本番row数・table sizeがリハーサル上限を
超える場合は標準migrationを直接実行せず、次のstaged手順へ切り替える。

1. edgeで同期write pathをquiesceし、旧serverが新しいproof / session rowを追加しない
   状態を確認する。read-only health確認は継続してよい。
2. ownerのautocommit sessionでmigrationと同じ順位規則により重複rowをcompactする。
3. 次を**transaction block外**で1文ずつ実行する。

   ```sql
   CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS
     continuity_closure_proofs_current_device_idx
     ON continuity_closure_proofs(tenant_id, device_id);

   CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS
     device_resync_sessions_current_device_idx
     ON device_resync_sessions(tenant_id, device_id);
   ```

4. `pg_index.indisvalid`と上記aggregate queryの`rows = devices`を確認する。失敗して
   `indisvalid = false`のindexが残った場合は、同期writeを再開せず人間判断で
   `DROP INDEX CONCURRENTLY`して原因を解消後に再実行する。
5. 同期writeをquiesceしたまま`taskveil-migrate`を実行する。migration側は同名indexを
   `IF NOT EXISTS`で認識し、cleanupを再確認してSQLx ledgerへchecksumを記録する。
   対応serverをdeployしてreadinessを確認した後にwrite pathを再開する。

`CREATE INDEX CONCURRENTLY`はSQLx migration transaction内では実行できないため、
migration SQLへ直接追加しない。write quiesce、staged index作成、再開はcredentialと
本番影響を扱う人間承認付き作業である。

## 7. 検証

最低限の検証:

```sh
cargo test -p taskveil-server
cargo test --workspace
git diff --check
```

APIレベルの検証:

- `/health` が成功する。
- `/ready` がruntime DB正常時200、停止・資格情報不正時503となり、response / logに接続情報を含まない。
- OPAQUE登録/ログインが成功する。
- push/pullがtenant分離、batch上限、blob上限、未来HLC拒否を維持している。
- 削除tombstoneの空blob方針が維持されている。
- application poolの `current_user` がnon-owner runtime loginで、`rolsuper = false`、`rolbypassrls = false`、`rolinherit = true`、`pg_has_role(current_user, 'taskveil_app', 'USAGE') = true`である。
- `_sqlx_migrations`の全行が`success = true`で、repository内のmigration versionとchecksumが一致する。
- runtime loginが`_sqlx_migrations`と`taskveil_schema_migrations`のSELECT / INSERT / UPDATE / DELETE権限を持たない。
- `tenants`、`tenant_members`、`tenant_seq`、tenant key generation / recipient、sync record/historyでRLSと`FORCE ROW LEVEL SECURITY`が有効である。List key tableは存在しない。
- tenant contextなしでは0行、tenant contextありでは当該tenantだけが見え、別tenantへのinsert/update/deleteが拒否または0件になる。

## 8. ロールバック方針

DB migrationは前方のみで扱う。失敗時は次の順に判断する。

1. migration適用前に失敗した場合: SQLを修正して再リハーサルする。
2. expand migration適用後にアプリが失敗した場合: 旧アプリが新schemaを無視できるならLambdaイメージだけ戻す。
3. データ補正が必要な場合: 補正SQLを新しいmigrationとして追加する。
4. 破壊的変更が入った場合: 人間判断でNeon backup/restoreを検討する。復旧手順と影響ユーザー判断はprivate側/人間管理とする。

## 9. 禁止事項

- 本番DBのconnection stringをpublicな場所に記録する。
- ユーザーの暗号blobを復号しようとする。
- リハーサルなしに本番DBへ破壊的SQLを適用する。
- private側で扱う判断事項をpublic repoのrunbookに書く。

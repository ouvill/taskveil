---
id: 019f9e39-aae9-7ae0-b53f-305c9a26f7e3
title: Home and calendar query benchmark
status: active
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #59では、Home / Calendarのproduction queryが複数のdue、
`scheduled_at`、`completed_at` rangeを扱う一方、実query plan、range index、
SQLCipherと実FRB経路を通る回帰benchmarkが不足している。

既存のFlutter large-data testはfake bridgeの描画時間だけを観測し、storage queryの
性能予算を持たない。既存migrationは変更せず、forward migrationと再現可能な
benchmarkでread性能とwrite amplificationを同時に管理する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/README.md` / `PLAYBOOK.md` / `STATUS.md`
- `docs/tasks/task-104-calendar-range-contract.md`
- `docs/03_技術仕様書.md`のlocal storage / FRB境界
- `core/storage/src/task_repository.rs`
- `core/storage/src/migrations.rs` / `core/storage/migrations/`
- `core/client/src/runtime/application.rs`
- `app/rust/src/api.rs`
- `app/test/performance_large_data_test.dart`

## 3. ゴール

- Home / Calendarのproduction queryをrange indexで検索可能にする。
- 10,000 taskのSQLCipher databaseで実query planとread/write性能を継続測定する。
- Flutterが利用する実FRB API経路に明示的なCI回帰閾値を設定する。
- index追加によるwrite amplificationを定量化して予算内へ固定する。

## 4. スコープ

### やること

- production queryと同じSQLに対する`EXPLAIN QUERY PLAN`をtestで記録・検証する。
- `scheduled_at` / `completed_at`とtyped due range向けindexをforward migrationで追加する。
- 必要に応じてOR predicateをsargableなqueryへ変更する。
- SQLCipherへ10,000 taskを投入し、Home / Calendarを実client / FRB APIから呼ぶbenchmarkを追加する。
- warm read性能予算、CI回帰閾値、index write amplification予算をcodeと文書へ明示する。
- migration upgrade、query semantics、benchmark fixtureをtestする。

### やらないこと

- 既存migrationの変更。
- Home / Calendarの表示契約、同期protocol、暗号形式の変更。
- fake Flutter描画testをstorage性能の正本として扱うこと。
- benchmark専用のproduction API追加。

## 5. 実装手順

1. 10,000 task seedで現行query plan、read latency、database size、insert latencyを計測する。
2. production queryをbranchごとにsargable化し、必要なpartial/range indexを設計する。
3. migration manifestへ連続したforward migrationを追加し、upgrade testを固定する。
4. storage query plan testとSQLCipher benchmarkを追加する。
5. app Rust bridgeのproduction APIを通るbenchmarkを追加し、CI向け閾値を設定する。
6. 全品質ゲート、benchmark反復、独立セルフレビューを実施する。

## 6. 受け入れ基準

- [x] Home / Calendar production queryの`EXPLAIN QUERY PLAN`に対象range indexが現れる。
- [x] 既存migrationを変更せずforward migrationだけでindexが追加される。
- [x] 既存Home / Calendar semantics testが維持される。
- [x] SQLCipherと実FRB API経路で10,000 task benchmarkが動く。
- [x] read性能予算とCI回帰閾値が明示され、自動testで超過を検出する。
- [x] index write amplificationの時間・database page増分を計測し、予算を超えない。
- [x] migration upgradeとfresh databaseの双方をtestする。
- [ ] 全品質ゲートと独立セルフレビューが合格する。

## 7. 制約・注意事項

- `core/storage/migrations/0001_initial.sql`を含む適用済みmigrationを変更しない。
- SQLCipherを迂回したSQLiteだけの計測を合格証拠にしない。
- query rewriteは重複task / occurrence、ancestor / descendant、sort順を変えない。
- wall-clock閾値はCI負荷を考慮しつつ、実測から乖離した形骸化した値にしない。
- benchmark fixtureは秘密情報や復号済み実ユーザーデータを使わない。

## 8. 完了報告に含めるべき内容

- migration番号、index定義、production query plan。
- 10,000 taskのHome / Calendar FRB latencyと性能予算。
- index前後のinsert latency / database page増分とwrite amplification。
- 実行した品質ゲート、commit、独立セルフレビュー結果、未解決事項。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 実行環境: macOS 26.5.2、Rust 1.97.0。Flutter CIはworkflow固定の
  Flutter 3.44.6を使用する。
- query: Home / CalendarのOR predicateを4本の`UNION` candidate branchへ分解し、
  duplicate taskはcandidate IDでdeduplicateした。Homeのancestor / descendant展開、
  Calendarのdual occurrence、half-open range、archived list contextは維持した。
- migration: `0002_home_calendar_range_indexes.sql`で旧
  `idx_tasks_home_targets`を置換し、active date due、active datetime due、
  active scheduled、closed completedの4 partial indexを追加した。`0001_initial.sql`
  は変更していない。
- query plan: Homeはtyped due indexをscanし、scheduled / completedをrange searchする。
  Calendarは4 branchすべてを対応するrange indexでsearchする。
- CI: Rust workspace testでstorage budgetを、macOS Flutter testでrelease版native
  libraryを通る実FRB budgetを毎回検証する。10,000件seedは
  `taskveil-client`の`test-support` exampleで別プロセス投入し、製品bridgeへ
  benchmark専用APIを追加していない。

計測結果:

| 経路 | Home | Calendar | 自動失敗閾値 |
|---|---:|---:|---:|
| Rust storage / SQLCipher / debug、5回median | 139ms（7,220 rows） | 33ms（5,836 rows） | 750ms / 250ms |
| Flutter → 実FRB → release Rust → SQLCipher、5回median | 71ms（5,834 rows） | 33ms（7,169 occurrences） | 1,500ms / 750ms |

index write amplification:

| 指標 | migration v1 baseline | migration v2 | 増分 | 自動失敗閾値 |
|---|---:|---:|---:|---:|
| 10,000件insert | 477ms | 498ms | +4.4% | baselineの150% + 500ms |
| DB論理サイズ | 7,057,408 bytes | 7,278,592 bytes | +3.1% | baselineの110% |

- 証拠:
  `home_and_calendar_production_plans_use_partial_range_indexes`、
  `sqlcipher_10000_task_query_and_index_write_budgets_hold`、
  `home_calendar_native_performance_test.dart`
- Commit: 未コミット
- 未解決: なし

### 独立検証

- 判定: 未実施
- 根拠: 実装担当外の検証者による全品質ゲートとdiff reviewを待つ。
- 検証者: 未割当

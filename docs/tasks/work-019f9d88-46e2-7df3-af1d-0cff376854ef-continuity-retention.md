---
id: 019f9d88-46e2-7df3-af1d-0cff376854ef
title: Bound continuity proof and resync session retention
status: active
lane: critical
milestone: maintenance
---

# Bound continuity proof and resync session retention

## 1. 背景とコンテキスト

GitHub Issue #58では、正常なpull closureごとに
`continuity_closure_proofs`へrowが追加され、ACK後も履歴として残るため、
同期回数に比例して増加する問題を扱う。ACK前proofだけは次のclosure発行時に削除されるが、
ACK済みproofにはretention境界がない。

統合候補`af8d55d`にはprotocol v9 page token hardeningが入り、
`device_resync_sessions`はfull resync restartごとに同じtenant / deviceの旧rowを削除して
1行へ置換する。技術仕様もdeviceごとに1行を正本としているため、この契約を維持し、
成功ACK時には完了済みsessionを削除する。

continuity proofはACK response lossに対する冪等再送を維持する必要がある。
そこでACK直後に削除するのではなく、tenant / deviceごとのcurrent proof 1行をupsertし、
同じhigh-water / generationのclosureでは同じproof IDとACK状態を再利用する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/09_運用ガイド.md`
- `docs/adr/ADR-010.md`
- `docs/adr/ADR-012.md`
- `docs/adr/ADR-016.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-95-fuzzy-scan-full-resync-gc-horizon.md`
- `docs/tasks/task-98-archive-first-deletion-sync-implementation.md`
- `server/src/sync.rs`
- `server/migrations/202607110002_archive_first_deletion.sql`
- `server/migrations/202607260003_resync_page_tokens.sql`
- `server/tests/sync_v2.rs`
- `server/tests/migrations.rs`

## 3. ゴール

- continuity proofとresync sessionのrow数を同期回数・restart回数から独立させる。
- ACK response loss、同一closure再取得、full resync restartの冪等性を維持する。
- 既存蓄積proofを前方migrationで安全にcurrent rowへcompactする。
- 保持境界と識別子を含まない監視指標を公開運用文書へ固定する。

## 4. スコープ

### やること

- `continuity_closure_proofs`へtenant / device単位のunique境界を追加する。
- 既存rowは未ACKを優先し、なければ最新ACK済みrowを残してcompactする。
- pull closureはcurrent proofをupsertし、同一high-water / generationではproof IDと
  ACK状態を再利用する。
- ACK transactionでcontinuityを単調更新し、対応する完了済みresync sessionを削除する。
- 長期同期、ACK再送、full resync restart、migration cleanupの回帰testを追加する。
- 技術仕様と運用ガイドへlifecycle retentionとaggregate監視指標を追記する。

### やらないこと

- sync wire、protocol version、暗号形式、client local schemaの変更。
- tombstone 180日保持、GC horizon、expired-device rebase意味論の変更。
- tenant / device識別子やrecord metadataをlog / metricへ追加すること。
- background scheduler、外部metrics provider、private運用情報の追加。
- 既存適用済みmigrationの変更。

## 5. 実装手順

1. 前方migrationで既存proofをtenant / deviceごとにcompactし、unique indexを追加する。
2. pullでcontinuity rowをlockし、current proofをupsertする。
3. 同一closureは既存proofを再利用し、異なるhigh-water / generationだけを置換する。
4. ACKのcontinuity更新、proof ACK、完了済みresync session削除を同一transactionに置く。
5. migration cleanup、長期normal sync、ACK retry、resync restartのbounded testを追加する。
6. retention契約とaggregate monitoring queryを仕様・運用文書へ記録する。
7. focused testからworkspace品質ゲートへ進み、親統合後に独立検証する。

## 6. 受け入れ基準

- [x] continuity proofはtenant / deviceごとに最大1行である。
- [x] 同じhigh-water / generationのclosure再取得は同じproof IDを返す。
- [x] ACK response loss後の同一proof再送が同じresponseとして成功する。
- [x] 新しいhigh-waterまたはgenerationはcurrent proofを置換する。
- [x] 既存蓄積proofは未ACK優先、次に最新作成時刻の規則で1行へcompactされる。
- [x] resync sessionはrestart時に1行へ置換され、成功ACK時に削除される。
- [x] continuity seq / generation、GC horizon、expired-device write guardを変更しない。
- [x] 長期normal syncとfull resync restart後もrow数がdevice数に対して有界である。
- [x] retention lifecycleとaggregate監視指標が運用文書に記録される。
- [x] wire / protocol version、client local schema、依存定義に差分がない。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace -- -D warnings`
- [x] `cargo test --workspace`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`
- [ ] 独立検証でP1 / P2相当の未解決指摘がない。

## 7. 制約・注意事項

- server schemaとcontinuity correctnessへ触れるためcritical laneとする。
  2026-07-26のユーザー指示により実装は承認済みである。
- ACK transaction以外で`continuity_seq` / `continuity_generation`を前進させない。
- current proofの置換とACKを競合させないため、同一deviceのcontinuity rowをtransaction中に
  lockする。tenant間RLS境界を維持する。
- proof ID、tenant / device ID、high-water、record metadataをlogや監視labelへ含めない。
- protocol v9 tokenの最大寿命24時間と5分margin、tombstone 180日保持を変更しない。
- 公開前のSecurity Advisory名、一時fork、private情報を文書・履歴へ含めない。
- 統合候補が未公開のためpush / PR / mergeを行わない。

## 8. 完了報告に含めるべき内容

- current proof upsertとmigration compactionの選択規則。
- ACK idempotencyとresync session削除のtransaction境界。
- 長期同期 / restart後のproof・session row数。
- 仕様・運用文書のretention / monitoring契約。
- 実行した品質ゲート、環境制約、commit hash、独立検証結果、未解決事項。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: closure proofをtenant / deviceごとのcurrent rowへ変更し、同一closureの
  proof ID / ACK状態再利用、異なるclosureでの置換、成功ACKと同一transactionでの
  完了済みresync session削除を実装した。既存proofは未ACK優先、なければ
  `created_at`と`proof_id`の降順で1件へcompactする前方migrationを追加した。
- 証拠:
  `long_running_sync_and_resync_restarts_keep_continuity_rows_bounded`で通常同期64回後の
  proof 1行とresync restart 64回後のsession 1行を確認した。
  `continuity_retention_migration_compacts_existing_proofs_before_unique_index`で未ACK優先、
  ACK済みのみの場合の最新row選択、unique境界を確認した。
  `server_trusted_continuity_binds_proofs_and_guards_all_writes`で成功ACK後の完了session
  削除を確認した。
- 品質ゲート: `cargo fmt --all -- --check`、
  `cargo clippy --workspace -- -D warnings`、`cargo test --workspace`、
  `sh app/tool/check_client_boundaries.sh`、`sh app/tool/test_client_boundaries.sh`、
  `git diff --check`はすべて成功した。Flutter変更がないためFlutter固有gateは対象外。
- Commit: この実装結果を含むcommit（最終hashはGit履歴を正本とする）。
- 未解決: 独立検証を親統合工程で実施する。実装・品質ゲート上の既知の未解決事項はない。

### 独立検証

- 判定: 未実施
- 根拠: 実装者とは別の検証枠が利用可能になった後、差分reviewと品質ゲート再実行を行う。
- 検証者: 未割当

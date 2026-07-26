---
id: 019f9d49-1b67-74b0-9417-95f381079619
title: Network operation guard partitioning
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #53 `[Architecture/P1] ネットワーク同期とローカルmutationの排他を分離する`
では、`sync_now`がprofile operation guardをnetwork await全体で保持するため、同じprofileの
exclusive account / key cutoverが遅いtransportに引きずられる問題を扱う。#55で通常operationは
shared guardとなりlocal CRUD同士は並行可能になったが、network workflowとprofile lifetime
coordinationの責務境界はまだ分離されていない。

syncの正しさはprofile shared guardの長期保持ではなく、DB-backed lease、fencing token、
runtime epoch、短いSQLite transaction、outbox `op_id` CASで保証する。Tenant key refreshは
remote fetchをtransaction外で行い、fetch後に発生したlocal mutationも含む最新row snapshot、
rotation backfill、key cache/generation/marker/epochを単一transactionへ閉じる。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/README.md`
- `docs/tasks/STATUS.md`
- `docs/tasks/work-019f9c87-9bff-7676-ae89-7f4a8d4c040e-profile-coordination.md`
- `docs/adr/ADR-019.md`
- `docs/adr/ADR-027.md`
- `docs/dev/client-profile-architecture.md`
- `core/client/src/runtime/mod.rs`
- `core/client/src/runtime/sync.rs`
- `core/client/src/runtime/account.rs`
- `core/client/src/runtime/application.rs`
- `core/client/src/runtime/recurrence.rs`
- `core/client/src/sqlite_sync_store.rs`
- `core/sync/src/apply/orchestration.rs`
- `core/sync/src/apply/pull.rs`
- `core/storage/src/profile_coordination.rs`

## 3. ゴール

- network await中にprofile shared / exclusive guardやSQLite transactionを保持しない。
- blocked sync transport中もcreate / update / deleteを成功させ、outbox headを欠落させない。
- sync single-flightはinstance statusとDB lease、cross-process安全性はfencing tokenで維持する。
- stale push ACKがnetwork待機中に置換された新outbox headへ作用しない。
- Tenant key fetch後のmutationを最新snapshotから新generationへbackfillする。
- dirty mutationを現在のdrainまたは1回のfollow-up syncへ確実に含める。
- cancel / error後にinstance running stateとDB leaseを回復する。

## 4. スコープ

### やること

- profile guardを短命なnetwork preparation guardへ分解し、runtime epochとDB key snapshotを
  network workflowへ渡した時点でOS/process guardを解放する。
- sync runはsnapshot epochでDB leaseを取得し、各HTTP直前と各commitでleaseを再検証する。
- Tenant key refreshをremote fetchとatomic local cutoverへ明確に分離する。
- key cutover transaction内で最新domain row列挙、rotation backfill、local key cache、
  pending marker、runtime epochを更新し、commit後にin-memory stateを再読込する。
- barrier、stale ACK、key fetch/cutover adversarial mutation、dirty follow-up、
  cross-process single-flight、cancel/error recoveryの回帰testを追加・補強する。

### やらないこと

- sync wire protocol、server schema、E2EE envelopeの変更。
- profile lock、runtime epoch、DB lease/fencingの削除またはSQLite-only排他への降格。
- network await中のSQLite transaction保持。
- account authentication saga全体の再設計。
- Flutter / FRB APIの変更。

## 5. 実装手順

1. Issue #53本文・根因コメントと#55統合コミットを照合する。
2. `sync_now`のprofile guard寿命を準備フェーズまでに限定し、epoch/key snapshotを明示する。
3. sync running stateとlease ownershipをRAII化し、normal/error/cancelの全経路で回復させる。
4. Tenant key remote fetchとlocal cutoverを分離し、cutoverをfenced single transactionにする。
5. outbox CASとdrain/follow-up条件を確認し、network待機中mutationの回帰testを追加する。
6. barrier/adversarial/cancel/errorとcross-process lease testを実行する。
7. workspace品質ゲートとclient boundary gateを実行する。
8. 独立検証へ渡せる証拠を完了報告へ記録する。

## 6. 受け入れ基準

- [x] blocked transport中にもcreate / update / deleteが成功する。
- [x] network await中にprofile OS/process guardとSQLite transactionを保持しない。
- [x] mutationのoutbox headが欠落せず、stale ACKは新しい`op_id`を削除しない。
- [x] sync中mutationが現在のdrainまたはdirty follow-upへ含まれる。
- [x] remote key fetch後、cutover前に発生したmutationが新generationでbackfillされる。
- [x] key cutoverのrow snapshot、rotation backfill、key cache、marker、epochが単一commitになる。
- [x] 同一instanceとcross-processのsync single-flightを維持する。
- [x] lease takeover後の旧ownerはHTTP開始、ACK、cursor、pull applyをcommitできない。
- [x] sync futureのcancel / transport error後にrunning stateとleaseを回復する。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace -- -D warnings`
- [x] `cargo test --workspace`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`
- [x] 独立検証でP1 / P2相当の未解決指摘がない。

## 7. 制約・注意事項

- typed readinessを含む公開済み`main`を起点とし、未公開のsecurity advisory差分や
  他Issue固有のcommitを履歴へ含めない。
- lock順序はprofile guard、session lockまたはsync lease、SQLite transactionを維持する。
- profile guardを外した安全性をinstance mutexだけで代替しない。cross-processの正本はDB leaseと
  fencing tokenである。
- remote side effectは送信後に取り消せないため、既存`op_id` CASとserver idempotencyを維持する。
- key materialはgeneration rollbackや同generationでのsemantic key変更をfail closedにする。
- task statusを`done`へ変更するには独立検証が必要である。

## 8. 完了報告に含めるべき内容

- profile preparation、sync running、DB lease/fencing、key cutover transactionの責務境界。
- blocked transport中CRUDとoutbox CAS / dirty follow-upの観測結果。
- key fetch/cutover adversarial test、cancel/error recovery、cross-process testの結果。
- 実行したRust workspaceとclient boundary品質ゲート。
- commit ID、変更ファイル、互換性影響、残る独立検証。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: `sync_now`はprofile shared guard内でDB keyとruntime epochをsnapshotした後、
  typed readinessとidentity bindingも同じ準備区間で一度だけ解決し、最初のnetwork await前に
  guardを解放する。sync single-flightはinstanceのrunning stateとDB-backed lease、
  remote/commit boundaryはowner・fencing token・取得時epochで維持した。
- Tenant key refreshはremote fetch / unwrapとlocal cutoverを分離した。local cutoverは
  profile guardを再取得せず、fenced `BEGIN IMMEDIATE` transaction内で最新domain rowを列挙し、
  rotation backfill、key cache、pending marker、runtime epochを一括commitする。commit後は
  account mutex下でdurable epochを再検証してinstance epochをCASし、競合したstale publisherを
  fail closedにする。旧leaseのrunは次boundaryで`LeaseLost`となる。
- dirty follow-up判定へdurable outbox headを追加し、follow-up後のempty-outbox readを
  sync完了のlinearization pointにした。既存`op_id` CASによりstale ACKは新headへ作用しない。
- 証拠: deterministic barrier中のready profileでcreate / update / deleteが成功し、
  coalesced tombstone outboxが残ることと短いexclusive cutoverが取得できることを確認した。
  remote key fetch後のmutationがgeneration 2で再backfillされるadversarial test、
  dirty outbox follow-up、future cancel時のrunning state回復、cancel / transport error時の
  lease解放testを追加した。既存のstale response、real child cross-process single-flight、
  expiry / takeover / fencing testもworkspace gateで通過した。
- 品質ゲート:
  - `cargo fmt --all -- --check`: pass
  - `cargo clippy --workspace -- -D warnings`: pass
  - `cargo test --workspace`: pass
  - `sh app/tool/check_client_boundaries.sh`: pass
  - `sh app/tool/test_client_boundaries.sh`: pass
  - `git diff --check`: pass
- 統合再検証: typed readiness候補上へのrebase後、client 120件、server unit 29件、
  server integration 40件、storage 88件（既定のperformance 1件はignore）、
  sync 103件、bridge 3件、client doc test 4件を含むworkspace全体を完走した。
- Commit: 本work itemを含む最終commit（最終hashはGit履歴を正本とする）
- 互換性: public Rust / FRB / wire / schema変更なし。sync中のexclusive operationは、
  長いnetwork await終了を待たず開始でき、epoch変更時は進行中runをfenceして次runへ渡す。
- 未解決: なし。

### 独立検証

- 判定: 承認。P1 / P2相当の未解決指摘なし。
- 根拠: typed readinessとの競合解消後、profile guard内でreadiness・DB key・epochを
  同時確定する境界、guard解放後のlease fencing、逆順lock取得がないkey cutover、
  durable dirty follow-up、cancel/error回復を差分と全workspace回帰testで再確認した。
- 検証者: Codex root（実装担当とは別の統合レビュー）

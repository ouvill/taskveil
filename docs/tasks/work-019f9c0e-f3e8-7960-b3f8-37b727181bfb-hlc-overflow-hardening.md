---
id: 019f9c0e-f3e8-7960-b3f8-37b727181bfb
title: Remote HLC overflow hardening
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

networkからdecodeしたHLC counterがlocal mergeで加算不能な場合、現在の実装は
panicし得る。同期recordは永続化・再配信されるため、remote inputは常に
fallible dataとして扱い、client/serverの両境界でpanic不能にする必要がある。

本作業はプロダクトオーナーが2026-07-26に承認したsecurity remediationである。
未修正の攻撃詳細は公開文書に記載しない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `SECURITY.md`
- `docs/03_技術仕様書.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-70-sync-server.md`
- `core/sync/src/hlc.rs`
- `core/sync/src/enqueue.rs`
- `core/sync/src/apply/records.rs`
- `server/src/sync.rs`
- `server/tests/sync_v2.rs`

## 3. ゴール

- remote HLCを処理する全経路からpanicを除去する。
- merge不能な値をserver ingressで拒否し、clientでも型付きに安全に失敗する。
- 認証deviceとHLC node identityの検証契約を明確にする。
- 同一profileの再認証でdevice UUIDが変わっても、旧nodeのpending writeを送信しない。
- base resyncのresponse loss、悪性page、token失効・rotationでcursorをpoisonせず、
  bounded restartにより収束するprotocolへ更新する。

## 4. スコープ

### やること

- HLC `now` / `merge`のoverflow契約を型として表現する。
- server push validationで受理不能なcounterを拒否する。
- client apply前のvalidation/quarantineまたはsafe failureを実装する。
- counter境界値とadversarial push/pull testを追加する。
- HLC future-skewをstrict boundaryとtyped retryable conflictとして定義する。
- HLC / outbox / account deviceのtransactional rebindを実装する。
- protocol v9の署名済みpage/completion token、明示ACK、24時間expiryを実装する。
- base scan tokenをURL queryへ載せず、bounded POST JSON bodyだけで送る。
- invalid / expired token時にlocal progress、marks、両tokenを原子的に破棄し、
  同一run最大1回だけ新generationへ再開始する。
- client/server migration、runtime secret、current/previous key rotation手順を更新する。

### やらないこと

- HLC encoding versionの全面変更。
- 5分というwall-clock skew幅そのものの変更。
- resync tokenをDB cursorや長期session credentialとして扱うこと。
- 脆弱性公開プロセスの変更。

## 5. 実装手順

1. HLC生成、observe、merge、server ingressの全call siteを列挙する。
2. remote inputを受けるAPIをpanic-freeな`Result`契約へ変更する。
3. serverが将来のhonest tickを不可能にするcounterを受理しない境界を定める。
4. authenticated deviceとHLC nodeのbindingを既存certificate/session契約と照合する。
5. account device変更時にquarantineを含む全outboxを新revision/op IDへ再発行する。
6. mutable server cursorをHMAC署名token chainとterminal ACKへ置換する。
7. tokenへserver-issued `issued_at` / `expires_at`を持たせ、24時間上限と
   current/previous overlapを検証する。
8. base scanをbounded POST bodyへ移し、旧GET/raw cursorを拒否する。
9. invalid/expired chainのatomic local resetとone-shot restartを実装する。
10. maximum counter、strict boundary retry、multi-relay、response loss、expiry、
    tamper、cross-tenant/device、rotation、migrationのtestを追加する。
11. 統合HEADで品質ゲートと独立security reviewを行う。

## 6. 受け入れ基準

- [x] network由来HLCで`panic!`、`expect`、unchecked overflowが発生しない。
- [x] `u32::MAX`とhonest tick headroomを失う値をserverが永続化しない。
- [x] clientは既存の悪性recordを受けてもprocessを終了しない。
- [x] safe failure後のretry方針と他recordへの影響が型・testで明確である。
- [x] HLC node identityがauthenticated deviceと矛盾する値を受理しない。
- [x] local monotonicityと既存のordering testが維持される。
- [x] maximum、maximum-1、wall dominant、counter dominantのtestがある。
- [x] future-skew境界ちょうどは全counterを拒否し、server clock進行後のretryで閉じる。
- [x] device再束縛はHLC、全outbox、account deviceを同一transactionで更新する。
- [x] page tokenはtenant/device/generation/base_seq/cursor/expiryへ署名束縛される。
- [x] page tokenはURLへ載らず、8KiB上限のPOST bodyだけで送られる。
- [x] terminal apply後にACKが失効してもatomic resetと最大1回のrestartで収束する。
- [x] client/server migrationとresync token secret rotation手順が定義される。
- [x] 対象test、workspace品質ゲート、独立検証が成功する。

## 7. 制約・注意事項

- malformed remote dataをlocal clockへ反映してから検証しない。
- overflow時にsaturating arithmeticで順序を曖昧化しない。
- pre-release方針に従い、必要ならwire validationをbreakingに強化する。
- 修正の公開は品質ゲートと独立検証の完了後にcoordinated disclosure手順で行う。

## 8. 完了報告に含めるべき内容

- HLC APIとerror typeの変更概要
- server/client validation順序
- device identity bindingの扱い
- 追加したadversarial test
- 品質ゲート、独立検証、公開準備の結果
- compatibility影響と未解決事項

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果:
  - `Hlc::now` / `Hlc::merge`を`Result`契約へ変更し、counter carry時に
    `wall_ms`も加算不能な場合は`CounterExhausted`を返してclockを変更しないようにした。
  - network counter上限を`u32::MAX - 2`とした。上限を進めるときは
    `wall_ms + 1` / `counter = 0`へchecked carryし、pull→observe→local mutation→
    push→別client observeでも単調性と再送可能範囲を維持する。
  - serverは全push opをtransaction開始前に検証し、counter上限超過、
    future-skew、top-level `revision_hlc` nodeと認証device UUIDの不一致を
    永続化前に拒否する。前deviceが発行した`base_revision_hlc`とmergeで保持する
    semantic clockのnodeはidentity一致判定から除外した。
  - clientはpush、delta pull、base scanのwire decodeで同じcounter上限を検証する。
    delta/base responseは全recordの検証後にだけpage型となるため、page中の後続悪性
    recordで失敗した場合も先行record、cursor、local HLCを変更しない。同じpageの
    retryはserver recordが修復されるまで同じtyped safe failureとなる。
    `observe_remote_hlc` / `tick_local_hlc_after`にも同じdecodeを置き、既存
    quarantine等から再構築されたrecordにもdefense-in-depthを維持した。
  - `SyncEngine::pull_page*` / `scan_base`だけがwire responseから`PullRecord`を
    構築し、そのprivate decodeで上限を検証する。apply側のraw decodeはこの検証後の
    pageまたは過去に検証後保存したquarantineを受け、さらに上記decodeで再検証する。
  - server統合fixtureの2-client経路を、同じsession tokenで異なるHLC nodeを
    模擬する構成から、実際に別device・別session tokenを登録する構成へ更新した。
  - future-skewはstrict `wall_ms < server_now + 5分`とし、境界ちょうどを
    全counterで一時拒否する。境界直前のhigh counterが1ms carryした場合は固定時刻で
    rejectし、server clockが1ms進んだretryで受理する。409 problem code
    `sync_clock_skew_retryable`をtyped分類し、有限counterの例外帯なしで複数relayが閉じる。
  - 同じlocal profileへ再認証してserver device UUIDが変わる場合、永続HLCと
    quarantineを含む全pending outbox headを新deviceへ単調に再発行する。
    HLC、outbox、`account_device_id`は同一SQLite transactionで更新し、
    account永続化が後続で失敗したretryでは同じdeviceへの再発行をno-opにする。
    sync開始時にもnetwork request前に同じrebindを行い、旧nodeのpushを防ぐ。
  - base scan cursorをserver DBで先行更新する方式を廃止し、protocol v9で
    tenant/device/generation/base_seq/stable cursorを束縛したHMAC-SHA256 page tokenへ
    変更した。同じtokenは同じpageを再取得でき、検証・適用・local cursor・次tokenを
    client transactionで原子的にcommitする。base scanはtokenをURLへ載せず、
    8KiB上限のPOST JSON bodyだけを使用し、旧GETとraw cursorを拒否する。
  - terminal pageはbase完了を直接publishせずcompletion tokenを返す。clientが
    terminal pageとtokenをdurable commitした後、明示ACKでserverの
    `base_complete`を冪等更新する。ACK応答消失時は同じtokenを再送でき、
    base ACK単独では独立したcontinuity proofを閉じない。
  - resync tokenは専用current/previous keyring、key ID、domain separation、
    canonical base64url、4KiB上限、server-issued時刻、24時間expiry、5分clock marginを
    持つ。派生tokenは最初のexpiryを継承する。tenant/device/old generation/tamper/skipを
    fail-closedにし、rotation overlap中はprevious tokenを検証しつつcurrent keyだけで
    発行する。release buildではtest key helperを公開しない。
  - invalid / expired pageまたはcompletion tokenの409を`ResyncRestartRequired`へ分類し、
    local progress、marks、両tokenを1 transactionで消去して同一run最大1回だけ
    新generationへ再開する。terminal適用後のexpiryでも再base scanから収束し、
    2回連続invalidなら停止して無限restartしない。
  - server migrationはprotocol v8のin-flight sessionと未ACK closure proofを破棄し、
    旧mutable cursor列を削除する。local migrationはv8のin-flight full resyncを破棄し、
    v9 token chainを新規開始する。再同期restart時は旧session rowも削除し、
    start response消失の反復でrowが無制限増加しないようにした。server migrationは
    既存のmigration sequenceと整合する`202607260002_resync_page_tokens.sql`、
    別DBであるclient SQLiteは`0003_resync_page_tokens.sql`とした。
- 証拠:
  - HLC unit testで`u32::MAX`、`u32::MAX - 1`、受理境界、wall優位、
    counter優位、clock不変、strict boundary、境界直前carry、+1ms retry、
    multi-relay、+1/+2ms attacker拒否を確認した。
  - client response testで、正常recordの後に悪性recordがあるpageを全体拒否し、
    同じretryもfail-closedになることを確認した。
  - server実DB adversarial testで、受理境界を含むbatchの原子拒否、
    `u32::MAX`、`u32::MAX - 1`、node spoof拒否、拒否recordの非永続化、
    `u32::MAX - 2`受理を確認した。
  - account testでsame-profile device再束縛、transaction rollback、account永続化失敗後の
    same-device retryでHLC/op IDを再発行しないこと、session tokenの最終publishを確認した。
  - resync unit/integration testでpoison page retry、terminal pageのdurable ACK待ち、
    page/ACK応答消失、token replay、tamper、cross-tenant、cross-device、old generation、
    24時間expiry、expiry restart、key rotation、bounded local reset、
    base ACKとcontinuity proofの分離を確認した。
  - server実DB `sync_v2`: 23件成功。
  - server migration test: 2件成功。
  - server unit test: 29件成功。
  - `cargo fmt --all -- --check`: 成功。
  - `cargo check --workspace --all-targets`: 成功。
  - `cargo clippy --workspace --all-targets -- -D warnings`: 成功。
  - `cargo test -p taskveil-sync -p taskveil-storage -p taskveil-client`: 成功
    （sync 102件、storage 83件成功・manual performance 1件ignored、client 64件成功）。
  - `cargo test -p taskveil-server`: 成功
    （unit・auth・billing・migration・realtime・RLS・sync integrationを含む）。
  - `sh app/tool/check_client_boundaries.sh`: 成功。
  - `sh app/tool/test_client_boundaries.sh`: 成功。
  - `git diff --check`: 成功。
  - Flutter変更はないためFlutter固有gateは対象外。
- Commit subject: `fix(sync): harden HLC and resync recovery`。
- compatibility影響:
  - HLC encodingは変更していないが、sync protocolは8から9へbreaking変更した。
  - 従来decode可能だったcounter `u32::MAX - 1` / `u32::MAX`と、認証deviceに
    一致しないtop-level revision nodeはnetwork境界で拒否される。
  - protocol v8の進行中full resyncはserver/local双方で再開せず、v9で安全に再開始する。
  - server起動には専用resync token current key ID/materialが必須となる。
- 公開準備:
  - 最新の統合対象に対する全品質ゲート再実行と、承認済みの
    coordinated disclosure手順による公開が残る。

### 独立検証

- 判定: 合格（blockerなし）
- 根拠:
  - 初回レビューで、有限carry bandの非合成性、token expiry不在、GET queryへの
    token露出の3件をrelease blockerとして検出した。
  - 修正版でstrict future boundaryとstable Problem Details、24時間expiry継承と
    atomic bounded restart、POST-only 8KiB body、server migration `003`を確認した。
  - 独立実行でstrict boundary 2件、POST/no-query 1件、token 5件、
    Problem Details 1件、SQLite v8→v9 migration reset 1件、
    `cargo fmt --all -- --check`、`git diff --check`が成功した。
- 検証者: quality_review（実装担当とは別エージェント）

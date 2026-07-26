---
id: 019f9e62-eaed-72e3-8c3c-a813a711a7c7
title: Shared profile process coordination
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #55 `[Architecture/P0] 共有profileのprocess間coordinationを実装する`
では、同一local profile databaseを複数の`TaskveilClient`またはprocessで開いたとき、
排他とruntime stateがinstance内に閉じている問題を扱う。

現状では、別instanceが古い`Anonymous` / `Ready` stateを保持し、account bind後も
outboxを作らずdomain rowを更新できる。Device Key rotation後には旧DB keyを保持した
instanceが残り、sync single-flightもprocess間では保証されない。SQLiteの
`busy_timeout`、短い`IMMEDIATE` transaction、migration ledgerだけでは、DB外の
secret store、network side effect、cached runtimeを含むworkflow全体を調停できない。

ADR-011はdesktop frontendが同じlocal profileを共有する方針とDB-backed sync leaseを
要求したが、lock hierarchy、runtime epoch、lease fencing、crash recovery、
OS別fail-closed契約は未確定である。本work itemはこれらを
[`ADR-027`](../adr/ADR-027.md)として先に固定してから実装する重要変更である。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/05_設計判断記録.md`
- `docs/adr/ADR-011.md`
- `docs/adr/ADR-013.md`
- `docs/adr/ADR-025.md`
- `docs/adr/ADR-026.md`
- `docs/adr/ADR-027.md`
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/work-019f9c0e-f3e8-7960-b3f8-37b727181bfb-hlc-overflow-hardening.md`
- `core/client/src/runtime/mod.rs`
- `core/client/src/runtime/account.rs`
- `core/client/src/runtime/application.rs`
- `core/client/src/runtime/sync.rs`
- `core/client/src/device_key_rotation.rs`
- `core/client/src/sqlite_sync_store.rs`
- `core/storage/src/database.rs`
- `core/storage/src/migrations.rs`
- `core/storage/src/transaction.rs`

## 3. ゴール

- 同じlocal profileを開く全frontendが同一のprocess間coordination契約を使う。
- profile binding、runtime crypto state、DB key、capsule generationのstale利用を
  mutationとsyncのcommit前に検出し、安全に再読込または失敗する。
- sync runをowner、expiry、単調fencing tokenを持つDB-backed leaseで1件に制限する。
- auth、migration、Device Key rotationをprofile-wide exclusive lockで直列化する。
- process crash、lease expiry、OS lock機能不在をfail-closedに扱う。
- CLI / MCPのshared profile実接続を、desktop各OSの実process検証完了までreleaseしない。

## 4. スコープ

### やること

- `taskveil-client`にprofile identityごとのprocess-local coordinator registryを設ける。
- canonical profile identityへ結び付くOS advisory shared / exclusive lockを実装する。
- lock順序を次へ固定する。
  1. process-local coordinator registryのshared / exclusive guard
  2. OS profile advisory shared / exclusive guard
  3. session credential lockまたはsync lease
  4. SQLite transaction
- profile bindingと単調`runtime_epoch`をSQLCipherへ永続化し、account/device bindingと
  Tenant Root Key cacheの実質的変更を発行するtransactionでepochを更新する。
- device再束縛はepoch発行前にlocal HLC、全outbox head、quarantine、account device
  markerを同一transactionで更新する。
- active capsuleの非秘密なgeneration markerを永続化し、generation変更時にDB keyと
  runtime stateを再open / 再読込する。
- sync leaseへランダムowner ID、expiry、単調fencing token、取得時runtime epochを持たせる。
- leaseのacquire / renew / releaseと、ACK、cursor、pull apply、full-resync stateの
  commit時fence検証を実装する。
- network待ちの間はSQLite transactionを保持せず、leaseを失ったownerは新規requestと
  local sync state commitを停止する。
- process crash後にOS lock、SQLite rollback、lease expiry、capsule recoveryを
  組み合わせて自動回復する。
- typed error、desktop/mobile別のfail-closed処理、実process testを追加する。
- Flutter / CLI / MCPへ、秘密や生profile pathを含まないbusy / retry情報だけを渡す。

### やらないこと

- 異なるPCやmobile / desktop間で同じ物理DBまたはDevice Keyをコピーして共有すること。
- SQLite transactionをnetwork request中ずっと保持すること。
- PID、lockfileの存在、wall clockだけを排他の正本とすること。
- lockfileを削除して強制unlockするstale-lock回復。
- productionでOS lockまたはsecret storeが使えない場合の平文・SQLite-only fallback。
- serverをlocal profile leaseの正本にすること。
- 常駐desktop daemon / brokerまたはlocal IPCへの全面移行。
- sync protocol、E2EE envelope、暗号suiteの変更。
- CLI / MCPのshared profile実接続を本work itemと同時に有効化すること。

## 5. 実装手順

1. ADR-027の人間承認を得て、ADR-011との差分、lock hierarchy、error taxonomy、
   OS fail-closed契約を確定する。
2. HLC hardening work itemを親として取り込み、device再束縛transactionと
   local migration番号を確定する。
3. `runtime_epoch`、capsule generation marker、sync lease / fencing tokenの
   前方migrationとstorage transaction primitiveを追加する。
4. canonical profile identityとprocess-local registryを実装し、相対path、
   symlink / junction、case差による同一profile lockの迂回を防ぐ。
5. macOS / iOS、Windows、Linux / AndroidのOS advisory lock adapterを実装する。
   lock APIや安全なprofile identity取得が利用不能なproduction環境では
   `ProfileLockUnsupported`としてopenを拒否する。
6. `TaskveilClient::open`をprofile exclusive guard内でcapsule recovery、
   migration、runtime loadするように変更する。通常operationはprofile shared guardを
   取得し、epoch / capsule generationを確認する。
7. auth、logout、binding publication、Device Key rotationをexclusive guardへ移し、
   secret storeとDBをまたぐ変更はpending / active markerによるdurable sagaで回復する。
8. local mutationをshared guardと短い`IMMEDIATE` transactionで実行し、transaction内で
   loaded epochを再検証する。不一致時はrollbackし、一度だけ再読込してretryする。
9. sync runへlease acquire / renew / releaseを組み込み、network response後の全commitで
   owner、fence、runtime epochを再検証する。remote side effectは既存の`op_id`、
   base revision CAS、冪等ACKにより再送安全性を維持する。
10. typed errorを追加する。
    - `ProfileBusy`: profile exclusive/shared競合
    - `SyncLeaseBusy`: 別ownerの有効lease
    - `LeaseLost`: expiry、takeover、runtime epoch変更
    - `ProfileLockUnsupported`: OS lock / canonical identityを安全に利用不能
    - `DatabaseBusy`: SQLiteのbounded busy timeout超過
    - runtime epoch不一致とcapsule generation変更は原則内部reload条件とし、
      再読込不能時だけ既存のaccount / crypto typed errorでfail closedにする。
11. macOS / Windows / Linuxの実process test、Android secondary-process
    instrumentation、共有containerを使うiOS extensionを対象にする場合の実process testを
    追加し、統合HEADで品質ゲートと独立検証を行う。

## 6. 受け入れ基準

- [ ] 同一profileへのprocess-local / OS lock順序が全call siteで一貫している。
- [ ] auth、migration、Device Key rotation中は別processのprofile operationが開始されない。
- [ ] stale `Anonymous` instanceがaccount bind後にoutboxなしのdomain mutationを
      commitできない。
- [ ] stale `Ready`、Tenant key、device ID、DB keyがruntime epochまたは
      capsule generation変更後に使用されない。
- [ ] account/device bindingとTenant Root Key cacheの変更が`runtime_epoch`と同じ
      transactionで発行され、device再束縛がepoch発行前にlocal HLC、quarantine、
      全outbox head、account device markerを原子的に更新する。
- [ ] 同一profileのsync leaseを同時に取得できるprocessは1つだけである。
- [ ] lease takeover後、旧ownerはACK、cursor、pull apply、full-resync stateを
      commitできない。
- [ ] owner process強制終了後、lockfile削除や手動修復なしにprofileを再openできる。
- [ ] SQLiteは未commit transactionをrollbackし、pending capsule / credentialは
      既存のdurable recovery手順で収束する。
- [ ] 同じprofileを相対path、symlink / junction、case差で指定しても同じlockへ収束する。
- [ ] 異なるprofileは別processから並行利用できる。
- [ ] OS lockが安全に利用できないproduction環境でSQLite-only動作へ降格しない。
- [ ] error、log、lock metadataへsecret、credential、復号済みcontent、生profile pathを
      出力しない。
- [ ] CLI / MCPのshared profile実接続がdesktop全対象OSの実process test完了まで
      release gateで拒否される。
- [ ] barrierで同期する実child process testが、stale owner、crash、lease expiry、
      同時mutation / sync、path aliasをsleep依存なしで再現する。
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cd app && flutter analyze`（Flutter / FRB変更時）
- [ ] `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release` の後
      `cd app && flutter test`（Flutter / FRB変更時）
- [ ] `sh app/tool/check_hardcoded_strings.sh`（Flutter変更時）
- [ ] `sh app/tool/check_client_boundaries.sh`
- [ ] `sh app/tool/test_client_boundaries.sh`
- [ ] `git diff --check`
- [ ] 独立検証でP1 / P2相当の未解決指摘がない。

## 7. 制約・注意事項

- 本work itemはP0であり、ADR-027は2026-07-26のユーザー明示指示により承認済みである。
- lock hierarchyを逆転させない。SQLite transaction内からprofile lock、
  session lock、sync leaseを取得しない。
- OS advisory lockは権威ある排他、owner metadataは診断専用とする。PID再利用、
  stale metadata、wall clock rollbackをlock解放の根拠にしない。
- Unixのsame-euid processとmobileの同一app container / App Groupをtrusted boundaryとし、
  group / world writableなprofile rootまたはDBはfail closedにする。権限を持つ悪意ある
  same-UID processによるrename、unlink、ptrace等は対象外とするが、open後のroot / DB
  identity driftはguard取得後に検出してoperationを拒否する。Windowsはroot handleを
  client lifetime中保持してdelete / rename sharingを拒否する。
- sync lease expiryはavailabilityのためのtakeover条件であり、fencing token検証なしに
  safetyを保証しない。
- local fencingだけでは送信済みHTTP requestをserverで取り消せないため、
  既存の`op_id`、CAS、冪等ACKを維持する。server-side fenceが必要になった場合は
  別のwire / server schema変更として裁定する。
- profile exclusive operationは長時間network処理を含み得る。timeoutやcancel後も
  durable markerを残し、次回openで回復が完了するまで通常operationを再開しない。
- HLC parent work itemのdevice再束縛transactionとnetwork前preflightを維持し、
  coordination実装で旧device nodeのoutbox送信を再導入しない。
- migrationは既存の適用済みSQLを変更せず、HLC親統合後の次の空きversionへ追加する。
- 本worktreeの変更はレビュー可能な単位でcommitし、公開前に統合HEADの品質ゲートを通す。
- public文書へprivate security情報、secret、未公開環境情報を記録しない。

## 8. 完了報告に含めるべき内容

- process-local registry、OS lock、session lock、sync lease、SQLite transactionの実装境界。
- runtime epoch / capsule generationのpublicationとreload手順。
- lease owner / expiry / fencing tokenのschema、更新条件、overflow / clock rollback処理。
- auth、migration、rotation、local mutation、syncのlock取得順。
- OS別fail-closed実装と対象外のplatform / process model。
- crash recoveryと実process testの観測結果。
- HLC/device-rebind parentとの差分とmigration番号。
- 実行したworkspace、Flutter、platform品質ゲート。
- independent reviewの判定、根拠、検証者。
- compatibility影響、release gate、未解決事項。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 結果: migration 0004、canonical profile coordinator、OS advisory lock、
  runtime epoch/capsule reload、mutation fence、sync fenced leaseを実装した。
  `TaskveilClient`はopen時のroot / DB identityとcoordinatorをlifetime中保持し、
  guard取得後のidentity driftをfail closedにする。Windowsはroot handleでdelete /
  rename sharingを拒否し、root、profile lock、session lock、DBのowner / DACLを
  handleから検証する。
- 証拠: stale Anonymous / Ready、lease contention / expiry / takeover / fencing、
  child process強制終了後のOS lock回復、sleepに依存しないauthoritative expiry /
  takeover、旧ownerのrequest / commit拒否、path alias、異なるprofileの実process並行性、
  stale root swap、同時mutation / sync testを追加した。実際の別processでstale clientを
  保持したままcapsule rotationとSQLCipher rekeyを行い、旧keyの拒否、capsule /
  DB keyの再読込、rotation後のmutation成功まで検証するtestも追加した。
  WindowsにはINHERIT_ONLY ACE、explicit child ACL、junction alias、root handle
  lifetimeのtestを追加した。`cargo test -p taskveil-client -- --nocapture`は
  110件とdoc-test 4件が通過した。
  HLC / resync hardeningと認可policyを含む統合HEADでも
  `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo fmt --all -- --check`、client boundary 2種、ADR structure、
  authorized sync route 2種、`git diff --check`が通過した。resync page tokenを受ける
  `/resync/base`と`/resync/base/complete`の双方へ8 KiBのrequest body上限を適用した。
  独立レビューで見つかった
  outer sync metadata、post-pull settlement、Tenant key cutover、profile identity pin、
  Windows child ACL、実processでのcapsule rekey/reload、completion endpointのbody
  limitの漏れには、active lease伝播、同一transaction fence、lifetime identity検証、
  handle-based security検証、追加の実process / HTTP testを追加した。
- Commit subject: `feat(client): coordinate shared profiles across processes`
- 未解決: macOS hostからのWindows cross compileはWindows SDK header不在により
  `ring` / `aws-lc-sys`で停止するため、Windows固有testの実行確認はGitHub Actionsの
  `windows-2025` matrixを正本とする。platform matrixの最終結果は公開可能な
  統合branchのCIで確定する。

### 独立検証

- 判定: APPROVE（P0-P3指摘なし）
- 根拠: 独立レビューのP0 / P1指摘はなく、残ったP2 2件へ実process testと
  request body limitを追加した。統合担当が`origin/main...2d8e257`の差分と
  security ancestryを再確認し、login privacy commitを含まないcurrent main直上の
  履歴であることを確認した。追加された実2-process testを独立再実行し、
  stale clientがcapsule rotation / SQLCipher rekey後に新keyを再読込してmutation
  できること、旧DB keyが拒否されることを確認した。Docker / PostgreSQLを使う
  resync統合testも独立再実行し、base / completionの双方が9 KiB bodyを413で
  拒否することを確認した。`git diff --check`にも合格した。Linux / macOS /
  Windows platform matrixは公開PRのCIで最終確認し、失敗時は本判定を取り消す。
- 検証者: 独立review agent、Codex root orchestrator

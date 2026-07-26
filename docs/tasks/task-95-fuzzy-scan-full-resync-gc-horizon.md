# task-95: Fuzzy-scan full resync / GC horizon

> ステータス: 完了（fuzzy scan・GC horizon・crash-safe mark-and-sweep・独立検証合格）
> 作業日: 2026-07-10

## 1. 背景とコンテキスト

ADR-010はtombstoneを180日後にGCし、テナントごとの`gc_horizon_seq`より古い非zero cursorをfull resyncへ送る方針を定めた。ADR-012はfull resyncを厳密な過去snapshotではなく、更新で移動しないstable keyによるcurrent-state fuzzy scanと`base_seq`後のdelta catch-upとして定義した。

task-92〜94により、Flutter / CLI / MCPの共通入口は`taskveil-client`の`TaskveilClient`へ集約され、Flutter bridgeからrepository・鍵・同期coordinatorが除去された。本taskではこの境界を維持したまま、GC後の端末と新規端末が、別端末の更新を止めず、欠落・未ACK local変更の消失・crash後の不整合なしにfull resyncできるproduction経路を実装する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/STATUS.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/03_技術仕様書.md` §6（同期、full resync、GC horizon）
- `docs/05_設計判断記録.md` ADR-010 / ADR-012
- `docs/dev/client-profile-architecture.md`
- `docs/tasks/task-92-client-profile-full-migration.md`
- `docs/tasks/task-94-rust-client-naming-cleanup.md`
- `core/sync/`、`core/storage/`、`core/client/`、`server/`の現行protocol・schema・同期実装と統合test

## 3. ゴール

- tombstone GC後と新規profileの双方で、安全かつ有限にfull resyncをclosureできる。
- base scan中も別端末の更新を止めず、scan中の作成・更新をdeltaで回収する。
- 未ACK outboxを持つlocal recordをsweepせず、serverに存在しない安全なlocal recordだけを除去する。
- base、delta、mark、sweep、cursor確定の各crash windowから同じresyncを安全に再試行できる。
- 既存local dataを新規tenantへ登録する経路でseed-before-sweepを守る。
- `TaskveilClient::sync_now`を唯一の高水準同期入口として維持し、Flutter / FRB公開call surfaceを変えない。
- strict snapshotやbase scan全体を覆う長時間DB lockに依存しない。

## 4. スコープ

### やること

- `taskveil-sync`: continuity / resync protocol型、stable-key cursor、base/delta page、high-water closure条件、state machineとstorage/server traitを実装する。
- `taskveil-storage`: resync generation、record mark、進捗、cursor、outbox保護付きsweepを支えるbreaking local schemaと短いtransaction primitiveを実装する。
- `taskveil-client`: SQLite adapter、preflight、seed、base scan、delta catch-up、mark/sweep、cursor確定、crash recoveryの実行順序を`TaskveilClient::sync_now`配下へ統合する。
- `taskveil-server`: server transaction内の`base_seq`取得、stable-key current-state page、GC horizon永続化とpreflight判定、delta rowsと同一transactionの`high_water`、closureに必要なAPIを実装する。
- protocol/server/local schemaは互換shimなしで正しい最終形へbreaking変更する。
- production 2-client経路と各crash windowを含む自動testを追加する。

### やらないこと

- aggregate削除scope / epoch、Canonical Inbox、server RLS hardening。
- Flutter/Dart公開API、FRB関数signature、画面、生成物の変更。
- `taskveil_app_bridge`へのrepository、鍵、resync coordinator、下位crate直接依存の追加。
- strict historical snapshot、scan全体を覆う長時間Postgres / SQLite transactionやDB lock。
- 互換shim、dual read/write、旧形式fallback、bare `core` crate。
- private repoの変更。

## 5. 実装手順

1. 現行protocol、server schema/query、local schema、`TaskveilClient::sync_now`の実行順序とテストfixtureを調査し、共有interfaceとmigrationの依存順を確定する。
2. `taskveil-sync`へpreflight判定、base/delta page型、stable-key cursor、`has_more=false`かつcursorがpage `high_water`へ到達した場合だけ成立するclosure条件を追加する。
3. `taskveil-server`へtenant sequence / GC horizon schema、server transaction内`base_seq`取得、`seq`で移動しないstable-key scan、`seq > base_seq` delta、同一transaction high-waterを実装する。base rowsは`seq <= base_seq`へ限定しない。
4. `taskveil-storage`へresync generation・進捗・mark・outbox保護付きsweep・closure cursor確定を短いtransactionで再試行できるschema/APIとして実装する。
5. `taskveil-client`でoutbox読取より先にpreflightし、`0 < since < gc_horizon_seq`と`since=0`を区別する。必要時はgenerationを再開/作成し、base、delta、closure、sweep、cursor確定、通常pushを順序づける。
6. 新規tenant binding時は既存local recordをtransactional seed/outboxへ登録してからresync/sweepし、未ACK outboxで保護されたrecordをclosure後の通常pushへ渡す。
7. 必須test、production 2-client収束test、FRB公開surface/boundary checkを実行し、全品質ゲートを統合HEADで通す。
8. 実装を担当していないエージェントが独立検証し、P1 / P2 / P3があれば修正と再検証を繰り返す。

## 6. 受け入れ基準

- [x] 空serverからのfull resyncがclosureし、最終cursorをclosure時high-waterへ設定する。
- [x] `since=0`はGC horizonが存在しても拒否されずfull resyncへ進み、非zero cursorだけが`0 < since < gc_horizon_seq`でcontinuity lossとなる。
- [x] `0 < since < gc_horizon_seq`ではlocal key bundle/entity outbox pushより前にfull resyncへ遷移する。
- [x] 最大active record seqがhorizon未満でもserver preflight / full resyncが正しく動作する。
- [x] base開始時にserver transaction内で`base_seq`を取得し、current stateを更新で移動しないstable keyでpage走査し、baseを`seq <= base_seq`へ限定しない。
- [x] page境界付近の同時作成・更新を欠落せず、base scan中に取り逃した変更を`seq > base_seq`のdeltaで回収する。
- [x] delta page rowsと同じserver transactionで`high_water`を取得し、`has_more=false`だけ、またはhigh-water未到達ではclosure扱いしない。
- [x] clientがresync generationを作成/再開し、base/deltaで確認したrecordをmarkする。
- [x] closure後、未ACK outboxを持つlocal recordはsweepせず、serverに存在せずmarkされなかった安全なlocal recordだけをsweepする。
- [x] base scan、delta、mark、sweep、cursor確定の各crash windowから再試行して同じ最終状態へ収束する。
- [x] 既存local dataの新規tenant登録でseed-before-sweepを守り、未push dataを失わない。
- [x] 2-client production経路がfull resync中の同時更新を含め最終的に収束する。
- [x] `TaskveilClient::sync_now`が高水準入口のままで、Flutter/FRB公開call surfaceとbridge boundaryが不変である。
- [x] strict snapshot、長時間DB lock、互換shim、dual形式、旧fallbackを追加していない。
- [x] `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、`cargo test --workspace`が成功する。
- [x] `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release`、`cd app && flutter analyze`、`cd app && flutter test`が成功する。
- [x] hardcoded strings、client boundaries、boundary negative test、`git diff --check`が成功する。
- [x] 独立検証でP1 / P2 / P3指摘がない。

## 7. 制約・注意事項

- 暗号blob、鍵、復号済みplaintext、session tokenをログ、test失敗表示、完了報告へ出さない。
- protocol、storage/client transaction、server transactionの責務を混在させず、Flutter bridgeへ同期実装を漏らさない。
- recordのcollectionは不変とし、stable keyは更新で移動しない値だけで構成する。
- `base_seq`はbase snapshot上限ではなくdelta開始境界である。base queryへ`seq <= base_seq`条件を加えない。
- closure前にabsence sweepを開始しない。closure cursorとpage `high_water`の一致を必須とする。
- sweepは未ACK `sync_outbox`をrecord単位で保護し、seed-before-sweepと同じ安全条件を使う。
- network I/OをSQLite transaction内へ置かず、server側もpage単位の短いtransactionとする。
- FRB生成物は手編集しない。公開signatureを変えないためcodegen差分を発生させない。
- Docker / Flutter SDKがsandbox制約で失敗した場合は、コード失敗と区別し、承認付き実行へ切り替えて再検証する。

## 8. 完了報告に含めるべき内容

- server/local schema versionと、追加・変更したtable / constraint / index / migration。
- protocol型、endpoint / trait、stable-key cursor、base/delta/high-water closureの具体的な契約。
- `TaskveilClient::sync_now`のpreflight、seed、resync、sweep、push順序とcrash recovery境界。
- 必須test名、2-client production test、全品質ゲートの実測結果。
- Flutter/FRB公開call surface不変とclient boundary維持の根拠。
- 独立検証の判定とP1 / P2 / P3、commit hash、未解決事項。

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-10
- 結果: `TaskveilClient::sync_now`配下へ、server強制continuity preflight、stable-key current-state base scan、`base_seq`後delta、高水位closure、durable generation/mark、outbox保護付きbounded sweep、crash recovery、seed-before-sweepを統合した。同期中の別端末pushはbase scan全体を覆うlockなしで継続できる。
- Server schema: migration `202607100002_fuzzy_resync.sql`で`tenant_seq.gc_horizon_seq BIGINT NOT NULL DEFAULT 0`を追加した。tombstone GCは削除行のtenant別最大seqでhorizonを同一statement内に単調前進する。
- Local schema: schema versionをv14からv15へ上げ、singletonの`sync_full_resync_state`とgeneration別`sync_full_resync_marks`、stable base/sweep cursor、`base_seq`、delta cursor、closure high-waterを追加した。
- Protocol/API: preflightへ`since`を必須化し、`since=0`はcapabilitiesを返し、`0 < since < gc_horizon_seq`は410を返す。410後も`since=0`でprotocol/envelope versionを再検証する。`POST /resync/start`、`POST /resync/base`、`PullResponse.high_water`、`StableRecordCursor`、`BaseScanResponse`を追加し、delta closureを`has_more=false && next_since==high_water`へ固定した。base endpointはprotocol v9で署名tokenを8KiB上限JSON bodyへ移し、旧GETを廃止した（2026-07-26追記）。
- Client transaction: `preflight → transactional seed → full resync → pending key bundle push → entity outbox push → normal pull`の順とした。base/deltaのapply・quarantine・存在mark・進捗、bounded sweep、最終cursor+generation cleanupはそれぞれ短いSQLite transactionで確定する。復号不能recordもserver存在としてmarkし、未ACK outbox recordと依存taskが残るlistをsweepしない。
- Crash recovery: generation開始、base apply/mark/progress、delta/closure、sweep batch、final cursorの各rollback後にdurable phaseから再試行するtestを追加した。新規tenant seedはoutboxとinitial-backfill cursorを単一transactionで確定し、seed commit前停止とsweep前保護を検証した。
- 証拠: `empty_resync_closes_and_base_scan_is_not_limited_to_start_seq`、`fuzzy_base_uses_stable_keys_and_delta_recovers_behind_cursor_changes`、`gc_horizon_can_exceed_max_active_seq_and_empty_delta_reaches_high_water`、`gc_horizon_full_resync_closes_before_local_outbox_push`、`continuity_410_still_enforces_protocol_upgrade_before_resync`、`full_resync_progress_and_marks_roll_back_together`、`full_resync_sweep_is_bounded_and_preserves_marks_and_unacked_outbox`、`transactional_seed_rolls_back_and_committed_seed_survives_absence_sweep`、production 2-client統合testが成功した。
- 品質ゲート: `cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、Docker統合14件を含む`cargo test --workspace`、bridge release build、`flutter analyze`、`flutter test`（130成功、visual QA harness 1 skip）、hardcoded string check、client boundary check / negative test、`git diff --check`が成功した。Rustでは既存のmacOS Keychain実物test 1件とtask-67手動performance test 1件がignoredである。
- 環境: sandbox内の初回Docker testはcontainer接続`Operation not permitted`、Flutter commandsはSDK cache更新`Operation not permitted`で失敗した。いずれもコード失敗ではなく、承認付き実行へ切り替えて同一ゲートの成功を確認した。
- Flutter/FRB: `app/`とFRB生成物は変更していない。bridgeの39公開関数signature test、release build、Flutter全test、境界guardが成功した。
- Commit: `3403b46`（`feat(sync): fuzzy-scan full resyncを実装`）
- 未解決: なし。aggregate削除scope / epoch、Canonical Inbox、RLS hardeningは契約どおり本task外である。

### 独立検証

- 判定: 合格（P1 / P2 / P3なし）
- 根拠: 実装非担当エージェントがADR-010 / ADR-012と全diffを照合し、stable-key base、delta/high-water、410後version再検証、push前遷移、quarantine mark、outbox保護、exact sweep、全crash window、seed-before-sweep、`TaskveilClient` / FRB境界を確認した。検証中のP2 2件（server側410判定不足、410時のversion検証迂回）は修正し、追加testと全品質ゲートを再実行後に最終合格した。
- 検証者: 実装を担当していないエージェント（independent_verifier）

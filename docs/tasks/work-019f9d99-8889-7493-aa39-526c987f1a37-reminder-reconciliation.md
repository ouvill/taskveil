---
id: 019f9d99-8889-7493-aa39-526c987f1a37
title: Durable reminder notification reconciliation
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

GitHub Issue #57で、reminderのDB mutation成功後にOS通知のschedule / cancelが失敗すると
UI操作全体が失敗扱いになり、DBとOS通知の状態が恒久的にずれる問題が指摘された。
起動時reconcileはreminderごとに全list / taskを探索して`runApp`前に待機し、
platform notification IDはUUIDの31bit hashだけで衝突を検出しない。

本作業はsettings boundaryまで統合したcommit
`b07b0028c9a590a07afd7f1427dc959f48617248`を基点とする。DBを唯一の正本、
OSローカル通知を再構築可能なderived stateとして扱い、domain mutationと通知pluginの
可用性を分離する。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-019.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-65-local-notifications.md`
- `docs/tasks/work-019f718e-a68a-7271-aa52-3b31bcc79348-multiple-reminders.md`
- `core/storage/migrations/0001_initial.sql`
- `core/storage/migrations/0004_settings_metadata_boundary.sql`
- `core/storage/src/migrations.rs`
- `core/storage/src/reminder_repository.rs`
- `core/storage/src/transaction.rs`
- `core/client/src/runtime/application.rs`
- `app/rust/src/api.rs`
- `app/lib/main.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/notifications/reminder_notifications.dart`

## 3. ゴール

- reminder / taskのDB mutationと同じtransactionで通知commandをdurableに記録する。
- OS通知失敗にかかわらず、commit済みのUI stateを即時反映する。
- reminder、task、list、platform IDを単一batch queryで取得する。
- 初回描画後にreconcileを開始し、アプリ起動を長時間blockしない。
- UUIDとplatform notification IDの永続一意mappingをDBで管理する。
- command version付きACKにより、並行更新後の新しいcommandを古い結果で消さない。

## 4. スコープ

### やること

- local migration `0005`を追加し、platform ID mapping、notification command outbox、
  reminder / task mutation triggerを導入する。
- SQLiteの一意な正整数allocatorをplatform IDへ使用し、hash衝突を設計上排除する。
- 起動時にDB正本からschedule / cancelのdesired stateを再構築する。
- Rust storage / client / FRBへbatch command取得とversion付きACKを追加する。
- Flutter通知serviceをsingle-flight workerへ変更し、成功commandだけACKする。
- providerと通知actionをDB-firstにし、plugin失敗をUI mutationから分離する。
- schedule / cancel失敗、process restart、占有済みID、stale ACK、JOIN context、
  初回描画非blockingのtestを追加する。
- 技術仕様を外科的に更新する。

### やらないこと

- server push通知、APNs / FCM、reminder同期protocolの追加。
- 通知本文へtask title / note等のコンテンツを追加すること。
- 適用済み`0001`〜`0004` migrationの変更。
- private repository、push、PR作成、main / candidateへのmerge。

## 5. 実装手順

1. v5 migrationへ永続mapping、versioned command、mutation triggerを定義する。
2. migration / repository testで既存reminder移行、JOIN、ID永続性、占有ID、
   stale ACKを固定する。
3. client / FRBへprepare、batch list、ACK APIとtyped DTOを追加する。
4. FRB codegenを実行し、Dart bridge / fakeを更新する。
5. notification serviceをDB command drainへ置換し、OS状態cleanupを初回だけ行う。
6. provider / screen / startupをDB-firstかつ初回描画後reconcileへ変更する。
7. failure / restart回帰と全品質ゲートを実行する。

## 6. 受け入れ基準

- [x] `LATEST_MIGRATION_VERSION`が5で、`0001`〜`0004`が不変である。
- [x] reminder作成・更新・snoozeはschedule、削除・task close / deleteはcancel、
      reopen / list移動はschedule commandを同じDB transactionで記録する。
- [x] platform IDはDBに永続化され、正整数・一意で、占有済みIDを再利用しない。
- [x] batch commandはtask / list contextをJOINで返し、reminderごとの全件探索を行わない。
- [x] OS API成功後だけ同じcommand revisionをACKし、stale ACKは新commandを消さない。
- [x] schedule / cancel失敗後もDB mutationとUI refreshは成功し、commandが再試行可能に残る。
- [x] transient失敗はforeground中のbounded exponential backoff、permission許可、
      foreground復帰、process restartで再試行し、background / dispose後にtimerを残さない。
- [x] orphan / noncanonical cleanup失敗はrebuild intentを保持し、同一serviceで再試行する。
- [x] 再起動時に未ACK commandとDB正本からOS通知状態を収束できる。
- [x] 初回reconcileは最初のFlutter frame後に開始する。
- [x] 新規OS payloadはowner + reminder IDだけとし、task / list ID、title、note、鍵、
      token等を含めない。旧3-ID payloadはdecode互換だけに限定する。
- [x] Rust API変更後にFRB 2.12.0 codegenを実行し、生成物を手編集していない。
- [x] 関連する全品質ゲートが成功する。

## 7. 制約・注意事項

- DB commitをOS notification APIの成功へ依存させない。
- command処理はschedule / cancelをidempotentに再実行できることを前提にする。
- platform IDはAndroidの正のsigned 32bit範囲に制限する。
- profile operation guard / runtime epoch / migration checksum ledgerを維持する。
- notification callbackとproviderから同時にreconcile要求されてもsingle-flightとする。
- unknown / legacy OS通知はpayload ownerを確認したものだけをcleanupする。

## 8. 完了報告に含めるべき内容

- parent / final commit SHA、migration番号と不変migrationの証拠
- mapping / command schemaとtrigger対象
- prepare / drain / ACK / restart lifecycle
- batch JOIN、初回描画非blocking、failure / collision / stale ACK test
- FRB codegenと生成物
- 全品質ゲート、skip、OS実通知を実行していない環境
- 未解決事項と独立検証状態

## 9. 完了報告

### 実装

- Parent commitは`b07b0028c9a590a07afd7f1427dc959f48617248`。既存
  `0001`〜`0004`を変更せず、forward migration `0005`を追加した。
- `reminder_notification_ids`でUUIDから正のsigned 32bit platform IDへの永続一意
  mappingとcommand revisionを管理する。cancel ACK後はmappingをretired reservation
  として保持し、ID再利用と起動ごとの不要なcancel再生成を防ぐ。
- `reminder_notification_commands`へschedule / cancelのdesired operationを保持する。
  reminderのinsert / update / deleteとtaskのstatus / deleted_at / list_id変更triggerが、
  domain mutationと同じSQLite transactionでcommandを更新する。
- repositoryは起動時にDB正本からdesired stateを再構築し、単一JOIN queryで
  reminder / task / list / effective schedule timeをbatch取得する。OS API成功後だけ
  `(reminder_id, revision)`一致でACKし、stale ACKは後続commandを削除しない。
- Flutter workerはsingle-flightでcommandをdrainする。schedule / cancel失敗時は
  commandを残し、foreground中の1、2、4、8、16、30秒bounded backoff、permission許可、
  foreground復帰、process restartで再試行する。background / dispose時はtimerをcancelし、
  1 foreground activationの自動試行回数を制限する。OS通知cleanup失敗もrebuild intentを
  保持して同じbackoffで再走査し、現在の通知IDにはUUID hashを使用しない。
- provider、snooze、同期後処理をDB-firstへ変更し、通知plugin失敗でcommit済みdomain
  mutationをrollbackまたは失敗表示しない。初回reconcileは`runApp`後の最初のframeで
  開始する。新規OS payloadはowner + reminder IDだけとし、task / list ID、title、note、
  鍵、token等を含めない。旧3-ID payloadは既存navigation / snooze / cleanupのdecode互換
  に限定した。
- Rust client / FRBへprepare、batch list、revision ACKのtyped APIを追加し、FRB
  2.12.0のconfig同値生成を行った。正規config-file commandはsandbox内のFlutter SDK
  cache書込制約により実行できず、独立検証側で生成差分を再確認する。

### テスト

- migration v4→v5、JOIN context、占有済みplatform ID、mappingの再open後永続性、
  stale ACK、cancel後retire、起動時再構築、task close / reopenをRust testで固定した。
- Flutter testへschedule失敗、cancel失敗、service restart、DB-first provider、
  permission denial、task lifecycle、orphan / 非canonical ID cleanup、snooze、
  first-frame後reconcileを追加した。独立レビュー後、schedule / cancel / cleanup失敗から
  同一service内でのbackoff回復、retry上限、background cancel / foreground resume、
  dispose、permission成功、最小payloadと旧3-ID decode互換を追加した。
- `cargo fmt --all -- --check`: PASS。
- `cargo clippy --workspace -- -D warnings`: PASS。
- `cargo test -p taskveil-storage`: 93件PASS、既存performance test 1件のみintentional
  ignore。
- `cargo test --workspace`: sandbox外の独立再実行で全件PASS
  （storage 93件 + performance 1件intentional ignore、client 123件、
  sync 103件、server unit / integration、bridge 3件を含む）。
- `env CARGO_TARGET_DIR=target cargo build --release`（`app/rust`）: PASS。
- Dart analyzerと`flutter analyze`: PASS。
- 初回の`flutter test test/reminder_notifications_test.dart`: 10件PASS。providerの
  fire-and-forget workerとfailure injectionが競合しないよう、restart testは
  domain mutationをbridgeへ直接commitしてからservice failureを1回注入する。
  独立レビュー修正後は同一service回復等を加えた15件とAndroid通知3件、合計18件が
  最終差分でPASSした。
- `flutter test`: 300件PASS、visual QA harness 1件intentional skip。
- config同値の`flutter_rust_bridge_codegen generate`を独立再実行し、生成差分なし。
- hardcoded strings、client boundary、boundary fixtureの3 scriptsと
  `git diff --check`: PASS。
- `0001`〜`0004`に対するbase commitとの差分がないことを確認した。

### Commit

- `85fe508 fix: reconcile reminders from durable state`
- 独立レビュー修正と本完了報告を含むfollow-up commit。

### 未解決事項・独立検証

- 実OS上の通知表示、permission lifecycle、アプリ強制終了を跨ぐ挙動はこの環境では
  実行していない。plugin gatewayと永続command境界をfailure injectionで検証した。
- 独立検証では初回に、同一process内の自動再試行欠落、cleanup失敗時のrebuild intent
  消失、OS payloadのtask / list ID過剰保持をP1 2件・P2 1件として指摘した。すべてを
  bounded backoff / lifecycle制御、cleanup再試行、current / legacy payload分離で修正した。
- 最終差分のreminder通知15件とAndroid通知3件は独立環境で全件PASSし、Dart analyzer、
  hardcoded strings、client boundary、boundary fixtureもPASSした。初回統合HEADでは
  full Flutter 300件PASS、visual QA 1件intentional skip、workspace Rust全件PASSを確認済み。
  最終修正はRust / FRB生成物を変更していない。
- 判定: 指摘3件は解消され、Issue #57の受け入れ基準に対して合格。

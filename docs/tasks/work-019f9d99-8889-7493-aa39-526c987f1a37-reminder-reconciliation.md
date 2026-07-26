---
id: 019f9d99-8889-7493-aa39-526c987f1a37
title: Durable reminder notification reconciliation
status: active
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

- [ ] `LATEST_MIGRATION_VERSION`が5で、`0001`〜`0004`が不変である。
- [ ] reminder作成・更新・snoozeはschedule、削除・task close / deleteはcancel、
      reopen / list移動はschedule commandを同じDB transactionで記録する。
- [ ] platform IDはDBに永続化され、正整数・一意で、占有済みIDを再利用しない。
- [ ] batch commandはtask / list contextをJOINで返し、reminderごとの全件探索を行わない。
- [ ] OS API成功後だけ同じcommand revisionをACKし、stale ACKは新commandを消さない。
- [ ] schedule / cancel失敗後もDB mutationとUI refreshは成功し、commandが再試行可能に残る。
- [ ] 再起動時に未ACK commandとDB正本からOS通知状態を収束できる。
- [ ] 初回reconcileは最初のFlutter frame後に開始する。
- [ ] payload / log /履歴へtask title、note、鍵、token等を含めない。
- [ ] Rust API変更後にFRB 2.12.0 codegenを実行し、生成物を手編集していない。
- [ ] 関連する全品質ゲートが成功する。

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
  commandを残し、再要求またはprocess restart後に再試行する。OS通知は固定owner payload
  のみをcleanupし、現在の通知IDにはUUID hashを使用しない。
- provider、snooze、同期後処理をDB-firstへ変更し、通知plugin失敗でcommit済みdomain
  mutationをrollbackまたは失敗表示しない。初回reconcileは`runApp`後の最初のframeで
  開始する。payloadとlogへtitle、note、鍵、token等を追加していない。
- Rust client / FRBへprepare、batch list、revision ACKのtyped APIを追加し、FRB
  2.12.0のconfig同値生成を行った。正規config-file commandはsandbox内のFlutter SDK
  cache書込制約により実行できず、独立検証側で生成差分を再確認する。

### テスト

- migration v4→v5、JOIN context、占有済みplatform ID、mappingの再open後永続性、
  stale ACK、cancel後retire、起動時再構築、task close / reopenをRust testで固定した。
- Flutter testへschedule失敗、cancel失敗、service restart、DB-first provider、
  permission denial、task lifecycle、orphan / 非canonical ID cleanup、snooze、
  first-frame後reconcileを追加した。
- `cargo fmt --all -- --check`: PASS。
- `cargo clippy --workspace -- -D warnings`: PASS。
- `cargo test -p taskveil-storage`: 93件PASS、既存performance test 1件のみintentional
  ignore。
- `cargo test --workspace`: 通知変更を含むtestはPASS。sandboxがlocal socket bindを
  禁止したため`taskveil-client`の既存HTTP server test 3件だけ環境起因で失敗し、
  非sandbox独立検証へ引き継いだ。
- `env CARGO_TARGET_DIR=target cargo build --release`（`app/rust`）: PASS。
- Dart analyzerによる`app`全体解析: PASS。`flutter analyze` / `flutter test`は
  Flutter SDK cache書込とlocal test socketがsandboxで拒否されたため、独立検証側で
  実行する。
- hardcoded strings、client boundary、boundary fixtureの3 scriptsと
  `git diff --check`: PASS。
- `0001`〜`0004`に対するbase commitとの差分がないことを確認した。

### Commit

- この完了報告を含むcommit。

### 未解決事項・独立検証

- 実OS上の通知表示、permission lifecycle、アプリ強制終了を跨ぐ挙動はこの環境では
  実行していない。plugin gatewayと永続command境界をfailure injectionで検証した。
- 独立検証担当が最終HEADに対して正規同値FRB codegen差分、対象・全Flutter test、
  sandbox外で必要な全Rust gateを再実行して合否を追記するまでは`status: active`を
  維持する。

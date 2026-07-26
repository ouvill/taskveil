---
id: 019f9c10-d2ef-70a2-9d18-43bbdbb0c788
title: Email ownership verification and transactional delivery
status: active
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

現在のaccount registrationはemailの形式と一意性だけを確認し、mailbox ownershipを
検証せずにuser、tenant、device、sessionを確定する。emailをlogin identifierとして
使う以上、account有効化前にmailbox controlを確認し、account enumerationを増やさず、
OPAQUE登録とE2EE key ownershipを混同しない登録protocolが必要である。

プロダクトオーナーは2026-07-26に、RFC、OWASP、NISTおよび著名な一次資料の
推奨へ従うこと、transactional emailの第一候補としてCloudflare Workers /
Cloudflare Email Serviceを評価すること、必要なdependencyとbreaking changeを
許可した。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `SECURITY.md`
- `docs/03_技術仕様書.md`
- `docs/05_設計判断記録.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-70-sync-server.md`
- `server/src/auth.rs`
- `server/src/config.rs`
- `server/src/lib.rs`
- `core/sync/src/account.rs`
- `infra/modules/deployment/*.tf`
- `realtime-worker/`

一次資料:

- OWASP Email Validation and Verification Cheat Sheet
- OWASP Authentication Cheat Sheet
- NIST SP 800-63A/63B confirmation-code guidance
- relevant email and URI RFCs
- Cloudflare Email Service / Workers API / domain authentication documentation

## 3. ゴール

- mailbox ownership確認後にのみdurable accountを作成する。
- registration request、verification、OPAQUE registrationを一回限りの短寿命ticketで
  結び、account existenceを外部へ開示しない。
- transactional email deliveryをprovider-neutralなserver boundaryへ置き、
  Cloudflare Workers Email Sendingを第一実装とする。
- email verificationとE2EE data/key recoveryを別契約として明記する。

## 4. スコープ

### やること

- pending email verification schema、expiry、single-use、attempt/retry制約を設計・実装する。
- generic registration request endpointとverification endpointを追加する。
- verification成功後にone-time registration ticketを発行する。
- OPAQUE register start/finishへticket bindingを追加する。
- email sender interfaceとCloudflare delivery adapter/Workerを実装する。
- anti-enumeration、rate limit、replay、concurrency、delivery failure testを追加する。
- RFC/OWASP/NIST/Cloudflare根拠と採用判断をADR・技術仕様へ記録する。

### やらないこと

- emailだけでE2EE暗号鍵や既存dataを復旧する機能。
- MFA、password reset、full account recoveryの同時実装。
- Cloudflare有料plan契約、本番domain onboarding、production secret投入。
- 開封tracking、marketing email、第三者analytics。

## 5. 実装手順

1. email canonicalization/comparison policyとanti-enumeration responseを確定する。
2. pending challenge、OTP keyed digest、expiry、attempt、consumed stateをschema化する。
3. verify成功後のone-time registration ticketとOPAQUE state bindingを実装する。
4. provider-neutral email commandとdelivery retry/idempotency境界を実装する。
5. Cloudflare Worker/Email Service adapterとlocal fakeを追加する。
6. client/FRB/UIをrequest → verify → OPAQUE finish flowへ変更する。
7. concurrent request、replay、expiry、known account、delivery failureを統合テストする。
8. staging設定手順とprovider切替手順を文書化する。

## 6. 受け入れ基準

- [ ] verification前にdurable user、tenant、device、sessionを作成しない。
- [ ] request、OTP verify、resend、expiry、register/startはaccount存在有無に
      かかわらず同じstatus/body/error shape、概算timing、観測可能な状態遷移を返し、
      known account用request_idも区別不能なdecoy lifecycleを持つ。
- [ ] OTPはCSPRNG生成、keyed digest保存、single-use、10分期限、5回上限である。
- [ ] verification ticketはnormalized email、challenge、OPAQUE registrationへbindingされる。
- [ ] client生成`handoff_secret`のS256 bindingにより、ticketをURLへ載せず元のappだけが取得できる。
- [ ] OTPは8桁、CSPRNG生成、keyed digest保存、10分期限、single-use、5回上限で、
      request IDとhandoff proofへatomicにbindingされる。
- [ ] `register/start`のidempotency resultはchallenge、purpose、request hash、
      Idempotency-Keyへbindingした暗号文として5分以内だけ保存され、raw ticketを
      平文永続化せずresponse lossから安全に再取得できる。
- [ ] resend、expiry、attempt上限、concurrent request、atomic consumeがtestされる。
- [ ] challengeと暗号化delivery outboxは同じPostgres transactionで確定し、dual-writeを持たない。
- [ ] deliveryはstable ID付きat-least-onceとして実装し、重複、期限切れ、retryable/terminal failureをtestする。
- [ ] 送信command Queue/DLQ/outboxに保存するrecipient、OTP、template dataは
      version付きAEADで暗号化される。
- [ ] Cloudflare lifecycle event Queueは送信command Queueと別のPII境界として扱い、
      初期版ではraw event subscriptionを無効にする。
- [ ] provider failureでaccount stateが半端に確定せず、安全にretryできる。
- [ ] sender domain、SPF、DKIM、DMARC、bounce/complaint運用前提を文書化する。
- [ ] Cloudflare dependencyをadapter境界の外へ漏らさない。
- [ ] `users.opaque_credential_id UUID UNIQUE NOT NULL`を持ち、register/startはticket由来のUUID bytesをOPAQUE credential identifierに使う。
- [ ] loginはcanonical emailをstable credential IDへ解決し、unknown emailでもfake OPAQUE responseを同じstatus/shape/概算timingで返す。
- [ ] login startは認証前にuser、tenant、device IDを返さず、pre-release baselineにemail credential IDとの二重経路を残さない。
- [ ] forward migrationはemail-bound OPAQUE recordを変換せず、既存の開発用account graphをatomic resetして再実行安全性とreset markerをtestする。
- [ ] email parserは単一addr-specのみを受理し、登録・login・変更・回復・招待で同じcanonicalization関数を使う。
- [ ] parserはASCII dot-atom local-part、IDNA A-label、明示的な長さ上限だけを受理し、
      quoted local、domain literal、末尾dot、EAIを拒否する。
- [ ] canonical emailの並行verificationはDB unique constraintとatomic reservationにより
      一つだけ成功し、競合結果からaccount存在を判別できない。
- [ ] 旧verification page、fragment token、ticket pollを残さず、元のapp内でOTPを
      入力し、成功後にだけpasswordを収集する。
- [ ] telemetryはMTA acceptedとinbox placement unknownを区別し、PIIをlogへ出さない。
- [ ] Workers Paid、Cloudflare DNS/domain onboarding、sender allowlist、quota、
      Email Preview無効、Queue retention、data locationをproduction release gateで
      検査し、不適合時は別provider adapterへ切り替える。
- [ ] email verificationがreal-world identityやE2EE key recoveryを意味しないと明記する。
- [ ] 対象test、workspace品質ゲート、独立security/architecture reviewが成功する。

## 7. 制約・注意事項

- OWASP/NISTより弱いOTP、expiry、anti-enumeration設計を独自採用しない。
- email local-partのcase foldingやplus-address除去を暗黙に行わない。
- raw OTP、email本文、password、OPAQUE state、session secretをlogへ出さない。
- verification URL、browser遷移、custom scheme、polling endpointを作らず、元のappへ
  8桁OTPを入力する経路だけを提供する。
- Cloudflare Email Sending Betaの利用可否をruntime capabilityとしてfail-closedに扱う。
- provider契約・課金・production secret操作は人間作業として残す。
- ADR-028の連番検査は先行するADR-027の統合後に成功させる。先行判断を複製したり、
  欠番を埋めるためのplaceholder ADRを追加したりしない。

## 8. 完了報告に含めるべき内容

- 採用した標準と設計判断
- endpoint、schema、ticket lifecycle
- canonicalizationとanti-enumeration policy
- email provider interfaceとCloudflare adapter
- retry/idempotency、rate limit、observability
- E2EE recoveryとの境界
- test、CI、独立検証、PR/merge結果
- staging/productionで残る人間作業

## 9. 実装記録

### 9.1 実装

- `register/request`、`register/resend`、`register/verify`、
  `register/start`、`register/finish`、`register/status`を短寿命challenge、
  handoff proof、one-time ticketで接続し、verification前のdurable account作成を
  廃止した。8桁OTPは10分、5回上限、generation単位のkeyed digest、resend commit時の
  旧generation即時失効をDB条件付き更新と制約でも強制する。
- OPAQUE credential identifierをemailから安定UUIDへ移行した。pre-release migrationは
  既存のemail-bound account graphを一度だけatomic resetし、reset markerとmigration
  再実行安全性をtestする。
- ASCII dot-atom local-partとIDNA A-label domainだけを受理する共通canonicalizationを
  registration、login、organization inviteへ適用した。local-partのcase foldingや
  plus-address除去は行わない。
- account存在時も同じHTTP shapeとdecoy lifecycleを進め、account/reservation競合は
  OPAQUE finish時にgeneric failureとする。canonical email reservationはrequest時に
  取得せず、verification transactionでrotation中のdigest候補全体を同順序にlockして
  直列化し、未確認requestによるmailbox lockoutを防ぐ。
- canonical digestとchallenge単位のdurable cooldown、35分windowの有限delivery budget、
  同形202 suppressionを実装した。配送recipientはcase-preserving local-partとA-label
  domainへ固定した。suppressed challengeのidentifier capacity昇格は実在challenge IDを
  lockする`SECURITY DEFINER`関数だけに限定し、runtime roleから任意digestを投入できない。
- OTP digest、handoff ticket、outbox command、idempotency responseは用途と対象へbindingした
  version付きHMAC/AES-GCMで保護し、current/previous key overlapを持たせた。AWS-only
  state keyringとWorker-shared delivery keyringを分離し、両keyring間の同一鍵値は
  startupで拒否する。
- challengeとencrypted delivery outboxを同一transactionで確定するprovider-neutral
  delivery境界を追加した。dispatcherはlease、期限、attempt上限、bounded concurrencyを
  持ち、retryable/terminal failureを分離する。
- `auth-email-worker/`へ8192 byte bounded reader、method/path/key ID/timestamp/body digest
  署名、Cloudflare Queue ingress lease、Email Service delivery lease、retention alarmを
  実装した。payloadを8桁OTPへ限定し、旧link/token fieldを拒否する。
- clientはremote request前にhandoff secret、origin、完全なrequest body、stable
  Idempotency-Keyを、remote start前にOPAQUE client state、完全なstart body、stable
  Idempotency-Keyを既存のDevice Key保護済みsession credential namespaceへ保存する。
  passwordはOTP待機中に収集・保存しない。remote finish前にはRecovery Key、local identity material、
  完全なfinish request、finish Idempotency-Keyを同journalへ確定し、response loss/
  process restart時は各phaseの同じrequestだけを再送する。journal遷移はcredential
  generation CASとし、mailbox待機中のpublic logout後に遅延responseがjournalを
  復活させない。PreparedFinishはghost account防止のためcancelを拒否する。
- verifyの成功とgeneric rejectionを同一tupleへ暗号化保存する。clientはOTP入力の
  keyed bindingをnetwork前にjournalへ保存し、応答ロスretryでは同じkeyを使い、
  入力変更・resend generation変更ではkeyをrotateするため、failed-attemptの
  二重消費と旧generation rejectionの誤replayを防ぐ。
- `register/status`はhandoff proofとstart/finish Idempotency-Keyで認証し、finishと
  同じadvisory lockで直列化する。24時間の暗号化receiptがあれば同じsession結果から
  local finalizeを再開し、確定的pendingなら専用CASでjournalを破棄して再登録可能にする。
  local finalizeは13か所のfault injection後もidentity、capsule、profile、sessionを
  冪等に収束させ、Recovery KeyはOS保護済み`RecoveryDisplayPending`としてUI ACKまで
  logoutを拒否して保持する。receipt失効後のlogin fallbackはprepared registrationと
  loginで復元したMK/account-root/Tenant Root DEK/generationのconstant-time一致を
  必須とし、既存accountへのdecoy登録で生成したRecovery Keyを誤帰属させない。
  refresh期限切れ後もRecovery Key表示・ACKを再開し、ACK後はaccount-bound identity
  guard付き再認証へ遷移する。
- `register/finish`はsource admissionと64KiB body limitをJSON decode前に適用し、
  idempotency key digest、request hash、AWS-only暗号化responseをaccount/session作成と
  同じPostgres transactionで確定する。
- EventBridge API Destinationからinternal dispatcherを定期起動するTerraformと、
  Worker/infra/Rust/Flutterを検査するCI jobを追加した。期限切れchallenge、
  idempotency結果、reconciliation receipt、delivery limitのGCは未認証request pathへ
  置かず、dispatcher起動時に各table最大128件ずつ処理する。

### 9.2 検証状況

- `cargo fmt --all -- --check`、SQLx offline check、
  `cargo clippy --workspace --all-targets -- -D warnings`、
  server以外のworkspace test（client 131、sync 106を含む）とserver全test
  （unit 53、auth 17、billing 9、migration 2、realtime 2、RLS 1、sync 24）は
  成功した。schemaを直接seedする既存server fixtureも新しいcanonical email /
  stable credential列へ移行し、pre-release reset markerとpost-reset accountの
  migration再実行安全性を検証した。
- Node.js 24.18.0でWorker typecheck、29 unit tests、dependency audit、Wrangler staging
  dry-run buildが成功した。旧link/token field、8桁ASCII decimal以外のOTP、
  unknown payload fieldをterminal rejectionとするtestを含む。
- OpenTofu staging/productionのformat、validate、module testと、Flutter analyze、
  全308 widget/integration tests（visual QA harness 1件はintentional skip）、
  release bridge build、client architecture boundary、UI hardcoded string check、
  `git diff --check`が成功した。
- 実装非担当の独立security/architecture reviewは最終
  P0 0 / P1 0 / P2 0 / P3 0で合格した。GitHub PR CIとmerge後main CIは統合担当が
  実施するため、このwork itemのstatusは`active`を維持する。

### 9.3 production release gate

- Cloudflare Email ServiceのBeta/Paid利用可否、送信domain onboarding、
  SPF/DKIM/DMARC、sender allowlist、quota、Email Preview無効化、Queue retention、
  data locationを人間が確認する。
- Worker、server、EventBridge connectionのproduction secretを投入し、fail-closedの
  placeholder senderを実domainへ置換する。email本文にURLを含めず、OTP template
  `verify-email-otp-v1`だけが送信されることをstagingで確認する。
- AWSだけへ投入する`TASKVEIL_EMAIL_STATE_KEY_*`と、AWS/Worker双方へ投入する
  `TASKVEIL_EMAIL_DATA_KEY_*`を別々に生成し、current/previousを含め同一鍵値を
  再利用しない。rotation時は両keyringを独立して更新する。
- 条件を満たさない場合はprovider-neutral adapterを維持したまま別providerへ切り替える。

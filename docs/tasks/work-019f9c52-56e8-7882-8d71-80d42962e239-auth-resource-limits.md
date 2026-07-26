---
id: 019f9c52-56e8-7882-8d71-80d42962e239
title: Bound public authentication resource consumption
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

未認証の OPAQUE registration / login start は、JSON body、暗号処理、DB lookup、
短命な中間状態の作成を伴う公開 endpoint である。前段の API Gateway throttling
だけに可用性を依存せず、アプリケーションと DB の双方で、1 request と同時に
保持する状態の最大コストを制約する。

認証結果からアカウントの存在を推測できる差異は先行 work item で是正済みである。
本 work item はその実装と独立した admission / capacity 境界を設け、既存の
known / unknown 差を拡大しない。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/03_技術仕様書.md`
- `docs/09_運用ガイド.md`
- `docs/adr/ADR-022.md`
- `docs/tasks/README.md`
- `docs/tasks/PLAYBOOK.md`
- `server/src/routes/auth.rs`
- `server/src/auth.rs`
- `server/src/config.rs`
- `server/tests/auth_server.rs`
- `infra/modules/deployment/api.tf`
- [OWASP Authentication Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Authentication_Cheat_Sheet.html)
- [OWASP Denial of Service Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Denial_of_Service_Cheat_Sheet.html)
- [RFC 6585 §4](https://www.rfc-editor.org/rfc/rfc6585#section-4)

## 3. ゴール

- start endpoint の body を JSON parse 前に制限する。
- process 全体、送信元 IP、正規化 identifier ごとの admission を有限にする。
- identifier は runtime secret で HMAC 化し、アカウント存在確認より前に同じ処理を
  適用する。
- DB に保持できる未完了 OPAQUE state を全体・identifier ごとに hard cap する。
- cleanup と in-memory limiter の bookkeeping を bounded にする。
- rate limit response を generic にし、アカウント存在に依存する情報を追加しない。

## 4. スコープ

### やること

- registration / login start 専用 body limit
- bounded token bucket と bounded key table
- trusted source IP の API Gateway → application contract
- HMAC key の runtime secret 読込
- OPAQUE state capacity counter migration とDB triggerによるtransactional claim / release
- burst、送信元分散、identifier parity、cleanup、oversized body の adversarial test
- 技術仕様・運用ガイドの公開可能な契約更新

### やらないこと

- private advisory、具体的な攻撃手順、production telemetry の公開
- Cloudflare zone resource の新規所有
- CAPTCHA、固定 account lockout、既存 login privacy branch の再実装
- production limit の負荷試験なしでの自動チューニング

## 5. 実装手順

1. admission limiter と HMAC identifier key を共有 application state に追加する。
2. start route に body limit と、DB lookup 前の admission 判定を配線する。
3. migration で全 OPAQUE state 共通の capacity lease / counter とinsert / delete triggerを追加する。
4. 旧 / 新codeのstate作成・消費・期限切れcleanupをDB triggerで同一transactionのleaseと整合させる。
5. API Gateway が送信元 IP header を上書きする contract を追加する。
6. adversarial test と品質ゲートを実行し、login privacy実装との統合結果を記録する。

## 6. 受け入れ基準

- [x] oversized / chunked body が JSON parse と DB work の前に `413` になる。
- [x] process 全体、送信元 IP、identifier の burst が有限の token bucket で拒否される。
- [x] identifier bucket key は secret-keyed HMAC で、known / unknown の判定経路に依存しない。
- [x] emailはASCIIに限定し、認証とlimiterが同じtrim + ASCII lowercase正規化を使う。
- [x] limiter table は上限を超えて増えず、1 request の cleanup scan 数も有限である。
- [x] DB の active OPAQUE state は全体・identifier ごとの hard cap を超えない。
- [x] state の consume / expiry cleanup が capacity counter を解放し、cleanup batch は有限である。
- [x] `429` の status / body は generic で、`Retry-After` は global / IP 制限だけに付く。
- [x] API Gateway の送信元 IP header は client 指定値を overwrite する。
- [x] Cloudflare zone resource を追加・変更せず、アプリ単体の global cap が残る。
- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace -- -D warnings`
- [x] `cargo test --workspace`
- [x] `sh app/tool/check_client_boundaries.sh`
- [x] `sh app/tool/test_client_boundaries.sh`
- [x] `git diff --check`

## 7. 制約・注意事項

- HMAC key、email、IP address、OPAQUE message / state をログへ出さない。
- source IP header は API Gateway が overwrite する場合だけ信頼する。将来 Cloudflare
  proxy を有効化する場合は、shared edge owner が origin bypass と client IP の信頼境界を
  同時に設計する。
- API Gateway / Cloudflare の rate limiting counter は厳密な global semaphore ではない。
  DB hard cap と process-global limiter を必須の最後の境界とする。
- login privacy実装との統合後も、admission が account lookup より前に残り、known /
  unknown で status、body、timing、DB work の差を増やしていないことをintegration
  gateの差分testで再検証する。
- login privacy実装がfailed proofでもOPAQUE state消費をcommitするため、capacity
  delete triggerによるlease releaseを同じtransactionへ残す。stateだけ、またはleaseだけを
  先にcommitせず、failed proofのconsume commitとtrigger releaseが同一transactionであるfailure injection
  test、および成功・失敗両finish後の件数不変条件testを必須integration gateとして再検証する。
- 新 migration は前方適用だけとし、既存 migration を編集しない。
- login privacy migration 002、resync page token migration 003、continuity retention
  migration 004を先行統合し、本変更は
  `202607260005_bound_opaque_auth_state.sql`として適用する。

## 8. 完了報告に含めるべき内容

- 各上限が保護する resource と、拒否時の response contract
- migration の既存短命 state 取扱い
- adversarial test 名と全品質ゲート結果
- Cloudflare shared zone と login privacy実装の統合結果
- 最終commitと独立検証の状態

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- 最新main統合・最終再検証: 2026-07-27
- 結果: registration / login startを、process-global / source admission、
  8 KiB body + JSON、HMAC canonical identifier admissionの順に制限した。in-memory
  key tableは4096 entryと32件scanに制限し、1 entry 1 queue nodeのrotation cursorで
  全entryを有限回に走査し、満杯時はshared overflow bucketへ送る。token refillは
  interval未満の端数を保持し、global / sourceの`Retry-After`は次refillまでの残り時間を
  秒単位で切り上げる。
  Postgresにはregistration / login共通の4096 state global counter、identifierごとの
  32 state counter、transactional leaseを追加した。SECURITY DEFINER / fixed search_pathの
  insert / delete triggerをcapacity invariantの正本とし、旧codeのNULL identifierはstate-local
  placeholderでglobal capを維持する。新codeは32-byte HMAC keyをstate INSERTへ渡す。
  claim、state insert、consume、expiry cleanupは同じstatement transactionでcounterと整合し、
  不足counterを0へ丸めずinternal errorでrollbackする。runtime roleはcapacity tableを直接更新できない。
- 応答契約: rate limitは`429 {"error":"too many requests"}`。`Retry-After`は
  account lookup前のprocess-global / source bucketだけに付け、identifier / DB capacity
  には付けない。oversized bodyはDBへ触れず`413`になる。
- ingress: API Gatewayが`x-taskveil-source-ip`をsource IPでoverwriteし、runtimeの
  explicit trust flagと組でだけ有効にした。直接到達、flag未設定、複数値、非canonical
  IPはpeer IPへfallbackする。Cloudflare zone resourceは変更していない。
- secret: runtime secretへbase64 32byteの`TASKVEIL_AUTH_LIMIT_HMAC_KEY`を追加し、非秘密の
  正整数generationをtyped configとstartup eventへ追加した。raw identifier、HMAC出力、
  key / key hashをlogへ出さない。rotationはauth start停止、最大TTL待機、bounded cleanupで
  active 0確認、secret / generation更新、新Lambda version発行、weighted oldなしのalias
  全量切替、旧invocation / concurrency収束確認、auth start再開の順にdrainする。
- migration: `202607260005_bound_opaque_auth_state.sql`。login privacyの002、
  resync page tokenの003、continuity retentionの004を先行させた統合HEADで再検証した。
  一般release前のため既存stateはmigration時に失効・削除し、strict per-identifier accountingを
  空状態から開始する。deployはauth ingress停止、migration、新version / alias全量切替、smoke、
  traffic再開の順とし、migration後rollbackは停止中だけ、原則はforward fixとする。
- 証拠: `malformed_json_is_source_limited_before_repeated_parsing`、
  `oversized_streaming_start_body_is_rejected_before_json_and_database_work`、
  `burst_and_distributed_sources_are_bounded`、
  `source_limited_attempts_do_not_consume_global_capacity`、
  `cleanup_queue_eventually_reaches_stale_tail_behind_hot_prefix`、
  `refill_preserves_fractional_interval_progress`、
  `known_and_unknown_identifier_limits_have_identical_http_responses`、
  `email_canonicalization_rejects_unicode_case_variants`、
  `opaque_capacity_claims_rollback_serialize_and_cleanup_in_bounded_batches`、
  `account_register_login_refresh_reuse_and_revocation_are_enforced`、
  migration test 2件が成功した。
- 品質ゲート: Rust fmt、SQLx offline check、workspace/all-target clippy、workspace test、
  client boundary 2本、OpenTofu fmt / bootstrap・staging・production validate、
  staging 1件 / production 3件のmock test、infra boundary、shell syntax、
  `git diff --check`が成功した。Flutter / FRB / UIは変更していないためFlutter gateは
  対象外。workspace testのreal Keychain 2件と10k performance 1件は既存のmanual
  ignored testである。login privacy / HLCとの統合HEADではserver全テスト、
  server all-target Clippy、OpenTofuの全対象validate / mock testを再実行し、
  known / unknownの応答同一性、single-use state consumeとcapacity releaseの
  同一transaction性を確認した。さらにpublic mainのIssue #75までを祖先に含むHEADで
  workspace test、workspace all-target Clippy、SQLx offline check、fmt、
  client / protocol / authorized-sync boundary、infra validate / mock testを再実行した。
- Commit subject: `fix(auth): bound authentication resource usage`。
- 未解決: 公開は先行するsecurity remediationと同じcoordinated disclosure手順で行う。
  critical laneの実装・PR・CI・mergeは2026-07-26にユーザー承認済み。

### 独立検証

- 判定: 承認（blockerなし）
- 根拠: 初回レビューで、source上限に達した同一送信元がglobal tokenまで消費する
  順序欠陥を検出した。sourceを先に判定するよう修正し、上記回帰testを追加した。
  最新main再構成時のレビューではPostgreSQLのUnicode lowercaseとlimiterのASCII
  lowercaseの差からidentifier keyを分割できる欠陥を検出した。email受付をASCIIへ
  限定し、route admissionと認証が共有normalizerを使うよう修正した。
  独立再実行でauth protection 8件、auth route 5件、config 10件、
  Unicode canonicalization 1件、Docker Postgres capacity invariant 1件、
  migration 3件、fmt / diff-checkが成功した。
  DB triggerの権限・rollback・並行・cleanup、trusted header境界、rotation /
  rollback runbookにも追加blockerがないことを確認した。最終HEADは最新public mainを
  祖先に含み、差分は本Issue固有の27 fileだけで、最終判定はP0〜P3すべて0である。
- 検証者: 独立レビュー担当 agent

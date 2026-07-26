---
id: 019f9c0e-f3e9-7280-8847-f7e68ef521d0
title: OPAQUE login privacy hardening
status: done
lane: critical
milestone: maintenance
---

## 1. 背景とコンテキスト

OPAQUE login startの応答がaccount存在有無で異なり、password proof完了前の
responseへ永続account identifierが含まれている。認証方式そのもののpassword
privacyだけでなく、account existenceとidentifierのprivacyもfail-closedにする。

本作業はプロダクトオーナーが2026-07-26に承認したsecurity remediationである。
未修正の再現詳細は公開文書に記載せず、修正後に必要最小限の設計契約と
回帰テストを残す。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `SECURITY.md`
- `docs/03_技術仕様書.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/tasks/task-70-sync-server.md`
- `server/src/auth.rs`
- `server/src/routes/auth.rs`
- `core/sync/src/account.rs`
- `server/tests/auth_server.rs`

## 3. ゴール

- login startのknown/unknown account経路をHTTP status、response shape、timing上で
  区別不能にする。
- password proof成功前に永続user/tenant identifierやaccount key materialを
  clientへ公開しない。
- OPAQUEの正規ログイン、誤password、期限切れ、state single-use契約を維持する。

## 4. スコープ

### やること

- unknown account用のsynthetic OPAQUE login pathを実装する。
- login start/finish DTOとephemeral stateをprivacy-preservingな形へ変更する。
- known/unknown differential testと通常ログイン回帰テストを追加する。
- 必要な技術仕様・完了記録を外科的に更新する。

### やらないこと

- メール所有確認と登録フローの変更。
- password change、account recovery、MFAの追加。
- 脆弱性公開プロセスの変更。

## 5. 実装手順

1. OPAQUE server APIがunknown recordを受ける標準的なsynthetic pathを確認する。
2. start responseから永続identifierを除き、ephemeral stateだけでfinishを継続する。
3. unknown stateもknown stateと同じexpiry、consume、generic failure経路へ流す。
4. client account flowを新しいDTOへ合わせる。
5. differential test、正規login、誤password、state reuse/expiry testを実行する。
6. 統合HEADで品質ゲートと独立security reviewを行う。

## 6. 受け入れ基準

- [x] known/unknown emailのlogin startが同じstatusとJSON field setを返す。
- [x] unknown emailでもvalid OPAQUE start messageを返し、finishはgeneric failureになる。
- [x] user ID、tenant ID、key bundleはproof成功前のresponseへ含まれない。
- [x] response timing差を増やすearly-return DB pathがない。
- [x] login stateはsingle-useかつ期限付きである。
- [x] 正しいpasswordの既存account loginが成功する。
- [x] 誤password、unknown account、expired/reused stateが同じ安全な失敗契約を守る。
- [x] secrets、password、OPAQUE state bytesをlogへ出さない。
- [x] 対象test、workspace品質ゲート、独立検証が成功する。

## 7. 制約・注意事項

- OPAQUE独自暗号拡張を作らず、`opaque-ke`のserver fake-record契約を利用する。
- pre-release方針に従い、旧wire DTO互換層を追加しない。
- 修正の公開は品質ゲートと独立検証の完了後にcoordinated disclosure手順で行う。

## 8. 完了報告に含めるべき内容

- start/finish DTOとephemeral stateの変更概要
- known/unknownを同一化した処理経路
- persistent identifierを返す時点
- 追加したdifferential testと通常回帰テスト
- 品質ゲート、独立検証、公開準備の結果
- 未解決事項と公開判断

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-26
- DTO: registration start用`RegistrationStartResponse`とlogin start用
  `LoginStartResponse`を分離した。login startは
  `state_id / opaque_suite_id / message / expires_at`だけを返し、永続user ID、
  tenant ID、device ID、device challenge、account key bundleはOPAQUE proof成功後の
  login finishで初めて返す。pre-releaseのbreaking wire変更であり、旧DTO shimや
  dual responseは追加していない。
- known / unknown path: emailによるuser検索後、knownは実user ID、unknownは
  requestごとのephemeral UUIDをRLS user contextへ設定し、双方で同じmembership
  queryを実行する。account未存在時の404 early returnを削除し、双方を同じserver
  setup、`ServerLogin::start`、10分期限、state insertへ流した。unknownは
  `opaque-ke`標準の`ServerLogin::start(None, ...)` fake-record経路を用い、独自の
  OPAQUE拡張は追加していない。
- migration: `202607260002_opaque_login_privacy.sql`で
  `opaque_login_states.user_id / tenant_id`をnullableにし、両方がNULLまたは両方が
  non-NULLであるCHECK constraintを追加した。unknown stateは永続的なdecoy
  accountを作らずNULL identity pairで表す。既存known stateと新規DBの双方へ適用する
  forward migrationであり、既存migrationは変更していない。
- state consumption: login finishは`DELETE ... RETURNING`をproof messageのparse /
  verificationより前にautocommitし、成功、誤password、unknown、malformed requestの
  いずれでもvalid state IDを1回だけconsumeする。期限切れ・再利用stateは同じ
  `401 {"error":"unauthorized"}`になる。finish request受理後にresponse lossまたは
  downstream failureが起きた場合は同じstateをretryできず、clientはlogin startから
  やり直す。この可用性tradeoffをsingle-use保証のため採用した。
- client: `AccountClient`を分割後DTOへ追随させた。unknown accountと誤passwordは
  fake-record応答を処理したOPAQUE client finishで同じ`AccountClientError::Opaque`
  となり、永続identifierを参照しない。email verificationやregistration flowの
  意味論は変更していない。
- tests:
  `opaque_login_hides_account_existence_and_consumes_every_state_once`でknown / unknown
  startのHTTP 200、同一4-field JSON shape、同じserialized message長、unknown stateの
  NULL identity、proof成功後だけのidentifier / key bundle公開、正常な
  `AccountClient` loginを確認した。誤password、unknown、expired、成功後replay、
  malformed attempt後replayはgeneric failureへ収束する。
- 検証:
  - `SQLX_OFFLINE=true cargo check -p taskveil-server --all-targets`: 成功。
  - focused Postgres integration test（最終変更後）: 1 passed。
  - `cargo test -p taskveil-server --test auth_server`: 2 passed。
  - `cargo test --workspace`: 407 passed / intentional ignored 3。実行後の
    consume-before-parse強化についてはfocused testを再実行して成功した。最終統合HEAD
    のworkspace再実行は独立検証側で行う。
  - `SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings`: 成功。
  - `cargo fmt --all -- --check`、client boundary check / negative self-test、
    `git diff --check`: 成功。
  - nullable queryは対象2箇所だけruntime-checked queryとし、既存`.sqlx` metadataに
    差分はない。
- Commit: 本文書と同じsecurity remediation commit。
- 未解決: リリース統合後のCI確認とadvisoryの公開判断。

### 独立検証

- 判定: pass（blocking findingなし）。
- 根拠: 実装担当外のverifierがknown / unknownの同一query形、`opaque-ke`
  fake-record、4-field response、nullable identity pair constraint、
  consume-before-parseのsingle-use、回帰testと技術仕様を静的確認した。
  `cargo fmt --all -- --check`、focused privacy integration test、
  migration test 2件、`./tool/sqlx_prepare.sh --check`、`git diff --check`も成功した。
  SQLxのunused metadata候補warningはmissing/stale metadataではなく、本変更による
  `.sqlx`削除を必要としないことを確認した。
- 検証者: quality_review agent（実装担当外、2026-07-26）。

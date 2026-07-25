---
id: 019f958b-23c6-77c2-a752-22e0c0ea9734
title: Standard session management
status: done
lane: critical
milestone: maintenance
---

# Standard session management

## 1. 背景とコンテキスト

現在の認証はRFC 9807 OPAQUEでパスワードを検証し、同時にE2EEのMaster Keyを解除するための`export_key`を得る。認証後は30日有効な単一opaque bearer tokenをDB照合しており、短命access token、refresh rotation、token reuse検知、標準revocation endpointがない。OPAQUEとE2EE鍵解除の境界は維持しつつ、認証後のセッション管理を広く採用されたOAuth仕様に沿う形へ置き換える。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/03_技術仕様書.md`
- `docs/adr/ADR-013.md`
- `docs/adr/ADR-020.md`
- `server/src/auth.rs`
- `core/sync/src/account.rs`
- `core/client/src/runtime/account.rs`
- `core/crypto/src/dev_key_store.rs`

## 3. ゴール

OPAQUEによる本人確認と端末側の鍵解除を維持し、API認可を短命access tokenとローテーション式refresh tokenへ移行する。refresh tokenの再利用を検知した場合は同じセッションfamilyを失効し、明示的logoutではRFC 7009互換のrevocation endpointを使用する。

## 4. スコープ

### やること

- 最大15分かつfamily absolute期限以内のopaque Bearer access tokenを導入する。
- 30日idle、90日absoluteのrefresh token familyを導入し、使用ごとにrefresh tokenをローテーションする。
- 消費済みrefresh tokenの再利用時にfamily全体を失効する。
- RFC 7009互換のtoken revocation endpointを実装する。
- RFC 8414互換のAuthorization Server metadataを公開する。
- mobile public clientをclient authenticationなしの固定client IDとして扱う。
- access token期限前にrefreshし、長時間実行されるsync / realtime requestでは401応答後1回に限り強制refreshして再試行できるようにする。
- 同一local profileを共有するprocess間でtoken setを変更するregister、login、refresh、logout、起動復元・期限切れ削除を直列化し、正規client同士の競合をreuseとして誤検知したりlogout後にtoken setを復活させたりしないようにする。
- 期限切れaccess tokenとfamily absolute期限を過ぎたsession rowを安全に回収する。
- access tokenとrefresh tokenを1つのversioned token setとしてOS安全領域へ保存する。
- server、Rust client、Flutter境界、インフラroute設定、技術仕様、回帰テストを更新する。

### やらないこと

- DPoPまたはmTLSによるsender-constrained tokenを導入しない。
- OPAQUEを外部ブラウザのOIDC Authorization Code flowへ移さない。
- OPAQUEの`export_key`をserver、browser、WebViewへ渡さない。
- JWT access tokenを採用しない。
- serverがE2EE鍵または復号済みcontentを扱う設計へ変更しない。
- OAuth authorization endpointや第三者client登録を提供しない。

## 5. 実装手順

1. ADR-025でtoken lifetime、rotation、reuse、revocation、storage、DPoP見送りを固定する。
2. session family、access token、refresh tokenのDB schemaを追加し、旧単一session tableを置換する。
3. OPAQUE完了時にtoken pairを発行し、access token認可、refresh rotation、reuse検知、revocation、metadataを実装する。
4. Rust clientのaccount sessionをversioned token setへ置換し、安全領域への保存と期限前refreshを実装する。
5. logout、sync、billing、key wrapper、realtime ticketの各経路を新しいaccess token lifecycleへ接続する。
6. refreshの正常系、並行・再利用、失効、期限切れ、未知token revocation、端末失効を自動テストする。
7. 統合HEADで品質ゲートを実行し、別担当による独立検証を行う。

## 6. 受け入れ基準

- OPAQUE登録・login後に最大15分かつfamily absolute期限以内のaccess tokenとrefresh tokenが発行され、E2EE Master Key解除が従来どおり端末内で完結する。
- access tokenだけでAPIを利用でき、refresh tokenはresource endpointへ送られない。
- refresh成功時に旧refresh tokenが消費済みとなり、新しいaccess tokenとrefresh tokenが返る。
- 消費済みrefresh tokenを再利用すると同じfamilyのaccess tokenとrefresh tokenがすべて拒否される。
- logoutはRFC 7009互換endpointでrefresh token familyを失効し、未知tokenでも200を返す。
- metadata documentからissuer、token endpoint、revocation endpoint、対応grantとclient authentication methodを取得できる。
- token setはApple Data Protection KeychainまたはAndroid Keystoreで保護された単一値として保存され、平文fileへ保存されない。
- access token期限前にrefreshし、sync / realtimeの401後は強制refreshと再試行を最大1回だけ行う。
- 同一profileの別process / instanceがregister、login、refresh、logout、起動復元を同時に開始しても、token setの変更を直列化し、旧refresh tokenの二重送信やlogout後のtoken再保存を起こさない。
- consumed refresh tokenをfamily absolute期限まで保持したうえで、期限切れaccess tokenと、期限切れfamilyおよび配下tokenが回収される。
- DPoP header、key pair、nonce管理を追加していない。
- repositoryの共通品質ゲートと独立検証が合格する。

## 7. 制約・注意事項

- `lane: critical` とし、2026-07-25のプロダクトオーナー指示を着手承認とする。
- refresh rotationは応答喪失時に再loginが必要となるfail-closed設計とし、grace windowや同一response再配布を追加しない。
- token本体、OPAQUE message、`export_key`、Device Key、復号済みplaintextをログ、error、完了報告へ含めない。
- access tokenとrefresh tokenはDBへhashだけを保存する。
- pre-release方針に従い、旧session APIとのdual read / writeや移行互換layerを追加しない。
- `docs/01_企画書.md` / `docs/02_機能仕様書.md`は変更しない。

## 8. 完了報告に含めるべき内容

- token lifetime、rotation、reuse検知、revocation、metadataの実装結果
- secure token set保存とrefresh/retry経路の実装結果
- 追加・更新したsecurity regression test
- 実行した品質ゲートと独立検証の結果
- DPoPを見送った境界と残るbearer tokenリスク
- 未解決事項

## 9. 完了報告

### 実装結果

- 作業日: 2026-07-25
- OPAQUE登録・login後の認可credentialを、15分のopaque Bearer access tokenと30日idle / 90日absoluteのrefresh token familyへ置換した。token本体は32 byte乱数とし、server DBにはSHA-256 hashだけを保存する。
- `application/x-www-form-urlencoded`のrefresh token grantを追加し、refreshごとの単回rotation、消費済みtoken再利用時のfamily全失効、端末失効時のfamily失効を同一transactionで扱う。
- RFC 7009互換の`POST /v1/auth/revoke`を追加した。refresh tokenはfamily全体、access tokenは当該tokenを失効し、未知tokenにもbodyなしの200を返す。
- RFC 8414互換の`GET /.well-known/oauth-authorization-server`を追加し、issuer、token / revocation endpoint、`refresh_token` grant、public clientのauthentication method `none`を公開する。
- clientはaccess / refresh token、期限、正規化済みissuer / resource originをversion付きの単一credentialとして保存する。production originはHTTPS、localhost / loopbackだけHTTPとし、refresh / resource / revokeの送信先をcredential側originへ固定する。Apple releaseはData Protection Keychain、Android releaseはKeystoreで保護されたstorageを使い、productionの平文file fallbackは持たない。
- access期限60秒前のsingle-flight refresh、sync / realtimeの401後に強制refreshして最大1回再試行する経路を追加した。refresh不能時はremote session credentialだけを破棄し、local crypto contextとoffline dataは保持する。
- 同一profileのregister、login、refresh、logout、起動復元・期限切れ削除を共通のcross-process advisory lockで直列化し、lock取得後にcredentialを再読込する。active / pending credential中のorigin変更を拒否し、別processが更新中の場合は旧refresh tokenを再送せずbusyを返す。
- logoutはremote revocationの200を成功条件とし、network / timeout / 非2xxではorigin-bound credentialを保持して再試行可能な失敗を返す。
- loginをOPAQUE finishのprovisional取得、profile identity照合、OS安全領域への`pending_device_certification`耐久保存、冪等certify、active finalizeへ分割した。crash / local保存失敗はpendingから再開し、identity不一致または未認証challenge期限切れはremote revoke後に破棄する。
- 認証・refresh request時の期限切れaccess token、absolute期限切れfamily、OPAQUE state回収を固定上限batchにし、未認証request一件へ無制限DELETE / cascadeを負わせない。consumed refresh tokenはfamily lifetime中のreuse検知に残す。
- DPoP header、端末DPoP key、proof、nonce処理は追加していない。access tokenはBearerのため、窃取時に最大15分replayされ得るriskが残る。
- Commit: このwork itemを含むPRのgit履歴を正本とする。

### 品質ゲート

- `cargo fmt --all -- --check`: PASS
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- `cargo test --workspace`: PASS（全workspace。Keychain実機2件とstorage performance手動test 1件はintentional ignore）。最終レビュー修正後にclient 57件、sync 89件、auth PostgreSQL integrationを再実行してPASS。
- `sh tool/sqlx_prepare.sh --check`: PASS
- `sh app/tool/check_client_boundaries.sh`: PASS
- `sh app/tool/test_client_boundaries.sh`: PASS
- `tofu fmt -check -recursive infra`: PASS
- `git diff --check`: PASS
- Rust bridge release build: PASS
- `flutter analyze`: PASS
- `flutter test`: PASS（280件成功、visual QA harness 1件はintentional skip）
- Android instrumentation Kotlin compile（JDK 21）: PASS
- iOS 18.2 / iPhone 16 Pro Simulator integration test: PASS（Data Protection Keychainを使うDK rotation後にSQLCipherを再open）
- Android 13 / Pixel 3a Emulator instrumentation test: PASS（3件。Keystore鍵の非export性、profile分離、`session-tokens`の保存・読込・削除）
- 認証integration testでtoken lifetime、token種別分離、rotation、reuse時family失効、登録session失効、未知token revocation、metadataを確認した。

### 独立検証

- 判定: 合格
- 根拠: 独立レビュアーが初回と再レビューで、origin未束縛credential、remote revoke失敗を隠すlogout、wrong-profile / crash時のcertified device orphan、secretの`Debug`露出、未認証hot pathの無制限GC、pending再開順序、provisional deviceのrotation混入、family revoke / cleanupの無制限fan-outを検出した。origin-bound credential、remote-first logout、耐久pending login saga、active-device gate、段階bounded GC、O(1) family revoke、redacted secret型へ設計変更後、client 57件、sync 89件、auth PostgreSQL integration、SQLx cache、check / fmt / diffを独立再実行し、P0〜P2未解決なしのPASSと判定した。
- 検証者: 実装を担当していない独立レビューサブエージェント。
- PR前再検証: 2026-07-25、新規独立レビューサブエージェントによるゼロベース再レビューと修正ループを完了し、最終PASS。

### 未解決事項

- Simulator / EmulatorではKeychain / Keystore経路を確認済み。Apple / Android実端末でのhardware-backed特性、process再起動後のtoken復元、refresh後のatomic置換はrelease実機gateでも確認する。
- DPoPを見送ったため、端末安全領域、TLS、ログ禁止、短いaccess lifetimeを破られた場合のBearer replay耐性はない。
- 新規登録は従来どおりserver commit後にlocal profileへ鍵とrecovery keyを保存するため、応答喪失や直後の端末故障ではpassword loginによるaccount recoveryと旧device revokeが必要になり、初回表示前のrecovery keyは再取得できない。loginで導入したprovisional saga / idempotencyをregistrationへも適用するか、login後のrecovery wrapper再発行UIを別critical work itemで設計する。
- login sagaの構成要素はclientのpending順序・credential復元testとserverの冪等certify・abandoned provisional cleanup統合testで固定した。process停止を注入し、`StoredPendingLogin`の再読込からcertify再試行、active最終publishまでを一つのclient/server harnessで通すfault-injection testは後続のrelease gateへ追加する。

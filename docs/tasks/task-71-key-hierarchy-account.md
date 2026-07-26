# task-71: P2-M3 鍵階層とアカウント接続

> ステータス: 完了（2026-07-08）
> 作成日: 2026-07-08
> 作業日: 2026-07-08

## 1. 背景とコンテキスト

Phase 2 P2-M3として、P2-M2で実装済みのOPAQUE/セッションAPIと、`docs/03_技術仕様書.md` §4の鍵階層をクライアントへ接続する。対象はクライアント側OPAQUEフロー、MK/KEK/DEK/Recovery Keyの生成・ラップ・復元、デバイス登録、ローカルKeychain保存、FRB公開、Flutterの最小アカウント画面である。

Taskveilの鍵設計は `docs/03_技術仕様書.md` §4を唯一の正とする。独自の鍵階層を発明してはならない。平文MKは永続保存せずメモリ上だけに置く。ローカル永続化するMK復元材料は `wrap(MK, DK)` であり、Keychain itemとして保存する場合も平文MKではなくラップ済み値を保存する。セッショントークンもKeychainへ保存し、Device Keyとはservice名を分離する。

P2-M3は同期ループそのものではない。登録/ログインで鍵が復元でき、以後P2-M4の同期エンジンがDEKを使える状態までを作る。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/README.md`
- `docs/tasks/BACKLOG.md`
- `docs/08_Phase2計画書.md` P2-M3
- `docs/03_技術仕様書.md` §1.5（OPAQUE中間状態、Postgres保存）、§3（users/devices/tenantとラップ済み鍵の置き場）、§4（鍵階層、各鍵の定義、保存場所、レコード暗号化方式）、§5.3（ローカルDB鍵）、§7（アカウントフロー）
- `docs/05_設計判断記録.md` ADR-003、ADR-007、ADR-008
- `docs/tasks/task-01-opaque-poc.md` の `## 9. 完了報告`
- `docs/tasks/task-64-keychain-device-key.md` の `## 9. 完了報告`
- `docs/tasks/task-69-sync-foundation.md` の `## 9. 完了報告`
- `docs/tasks/task-70-sync-server.md` の `## 9. 完了報告`
- `server/src/auth.rs`
- `server/src/routes/auth.rs`
- `server/src/db.rs`
- `server/migrations/202607080001_sync_server.sql`
- `core/crypto/src/aead.rs`
- `core/crypto/src/kdf.rs`
- `core/crypto/src/opaque.rs`
- `core/crypto/src/device_key.rs`
- `core/sync/src/envelope.rs`
- `app/rust/src/api.rs`
- `app/rust/src/dev_key_store.rs`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/router.dart`
- `app/lib/src/screens/lists_screen.dart`

## 3. ゴール

- `core/sync`（または同等の共有Rust crate）に、`opaque-ke 3.0.0` のクライアント登録/ログインフローと `reqwest(rustls)` HTTP clientを実装し、Flutter/CLI/MCPが再利用できる形にすること。
- 登録時にMK、Recovery Key、User X25519鍵ペア、個人Tenant Root DEK、既存個人リスト用List DEKを生成し、`docs/03` §4どおりにラップしてサーバー/ローカルへ保存すること。
- ログイン時にOPAQUE exportKey由来KEKで `wrap(MK, KEK_pw)` を復号し、MK、User X25519秘密鍵、Tenant Root DEK、List DEKを復元できること。
- 新デバイスログイン時に `devices` 表対応のデバイスメタデータを登録し、ローカルDevice Keyで `wrap(MK, DK)` を作ってKeychainへ保存すること。
- セッショントークンと `wrap(MK, DK)` をKeychainへ保存し、service名をDevice Keyとは分離すること。平文MK/KEK/DEK/exportKeyはログ出力せず、メモリ上では `zeroize` で破棄すること。
- FRBに `register` / `login` / `logout` / `session state` 系APIを公開すること。MK/KEK/DEK等の鍵バイト列をDart境界へ出さないこと。
- FlutterにLists画面overflowから「アカウント」導線を追加し、未ログイン時の登録/ログイン、ログイン済み時のメール表示/ログアウト、デバッグ用サーバーURL設定を最小UIとして実装すること。
- testcontainers Postgres + 実サーバー起動 + Rust clientで、登録→ログアウト→ログイン→MK復元の一気通貫統合テストを通すこと。

## 4. スコープ（やること・やらないこと）

### 想定変更ファイル

- ルート `Cargo.toml`
- `core/crypto/Cargo.toml`
- `core/crypto/src/*.rs`
- `core/sync/Cargo.toml`
- `core/sync/src/*.rs`
- `server/Cargo.toml`
- `server/src/auth.rs`
- `server/src/routes/auth.rs`
- `server/src/db.rs`
- `server/migrations/*.sql`
- `server/tests/*.rs`
- `app/rust/Cargo.toml`
- `app/rust/src/api.rs`
- `app/rust/src/dev_key_store.rs`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/router.dart`
- `app/lib/src/screens/lists_screen.dart`
- `app/lib/src/screens/account_screen.dart`
- `app/lib/l10n/app_en.arb`
- `app/lib/l10n/app_ja.arb`
- `app/test/**/*`
- `docs/tasks/task-71-key-hierarchy-account.md`
- `docs/tasks/README.md`
- `docs/tasks/BACKLOG.md`

### やること

1. `git status --short` で作業前状態を確認する。未コミット変更があれば、タスクに関係するものだけを慎重に読み、無関係な変更は触らない。
2. Rust依存はworkspace集約で追加する。想定は `reqwest`（rustls、default-features off）、`zeroize`、必要に応じて `x25519-dalek`、Recovery Key生成用crate、base64/uuid補助である。既存依存で足りるものは追加しない。
3. `core/crypto` に鍵階層用の小さなプリミティブを追加する。MK/Tenant Root DEK/List DEKは32byte乱数、KEK_pwはOPAQUE exportKeyからHKDF-SHA256、Recovery Key由来鍵も固定context付きHKDFで32byteへ導出する。wrap/unwrapは既存AEADを使い、用途別AAD/contextを定数化して互換性テストで固定する。
4. `core/sync` にaccount client moduleを追加する。OPAQUE client start/finish、register/login/logout/session refreshに必要なHTTP DTO、base64変換、server URL設定、セッション保持抽象をRust側に置く。ネットワークはDartではなく `reqwest(rustls)` で行う。
5. サーバーmigrationを追加し、`docs/03` §4.3/§4.9のサーバー保存物を保持できるようにする。最低限、`wrap(MK, KEK_pw)`、`wrap(MK, RecoveryKey)`、User X25519公開鍵、`wrap(SK, MK)`、個人Tenant Root DEKのwrap、個人List DEKのwrapを保存/取得できる列またはテーブルを用意する。
6. OPAQUE登録finishを拡張し、クライアントがregistration finish messageと同時に鍵payloadを送れるようにする。サーバーはラップ済み鍵と公開鍵だけを保存し、パスワード、exportKey、MK、KEK、DEK、Recovery Key平文を受け取らない。
7. OPAQUEログインfinishを拡張し、セッションレスポンスに復号に必要なラップ済み鍵bundleを返す。クライアントはexportKey由来KEKでMKを復元し、MKでUser X25519秘密鍵/Tenant Root DEK/List DEKを復元する。
8. デバイス登録を `docs/03` §3.2/§7.3に合わせる。ログインfinish時にdevice_nameと必要ならdevice public_keyを保存し、サーバーには `wrap(MK, DK)` を送らない。`wrap(MK, DK)` はローカルKeychain保存とする。
9. `app/rust/src/dev_key_store.rs` のtask-64パターンを一般化し、Device Key、session token、local wrapped MK用にservice名を分離したKeychain storeを用意する。Flutter test時は既存と同様にファイル/メモリ系fallbackを使い、実Keychainへ触れない。
10. FRB公開APIを追加する。現在の登録APIは `accountRegistrationBegin`、`accountRegistrationVerifyOtp`、`accountRegistrationComplete` の段階型フローとし、`accountLogin(...)`、`accountLogout()`、`getAccountSessionState()`、`get/setSyncServerUrl()`を公開する。関数名は既存Dart namingに合わせる。Rust API変更後はリポジトリルートで `flutter_rust_bridge_codegen generate --config-file flutter_rust_bridge.yaml` を実行し、生成物は手編集しない。
11. Flutter UIを追加する。`/account` route、`AccountScreen`、Riverpod provider/fake、en/ja ARBを実装する。Lists画面右上overflowに「アカウント」導線を置く。未ログイン時は登録/ログインフォーム、ログイン済み時はメール+ログアウトを表示する。Recovery Keyは登録完了後に一度だけ表示し、画面遷移後に再表示できないようにする。
12. デバッグ用サーバーURLをsettings storeに保存する。キー名は既存 `ui_mode` と衝突しない `sync_server_url` 等にし、未設定時は `http://localhost:3000` を既定値にする。通常利用時に秘密情報をsettingsへ保存しない。
13. Rust統合テストを追加する。testcontainers Postgresにmigrationを当て、実axum serverをローカルportで起動し、Rust clientから登録→セッション確認→ログアウト→ログイン→同一MK復元を検証する。誤パスワード、失効セッション、誤鍵unwrap失敗も含める。
14. Flutter widget testをfake bridgeで追加する。未ログイン画面、登録成功後のRecovery Key一度表示、ログイン済みメール表示、ログアウト後の未ログイン復帰、サーバーURL設定の表示/保存を確認する。
15. 秘密情報ログ禁止を確認する。`println!`、`dbg!`、`tracing`、Flutter error表示にパスワード、session token、MK、KEK、DEK、DK、exportKey、Recovery Key平文を含めない。

### やらないこと

- P2-M4の同期ループ、push/pullの自動実行、outbox ACK、pull復号マージ、UI invalidate連携。
- Recovery Keyによるパスワード復旧UX完全版。P2-M3では登録時生成、一度だけ表示、`wrap(MK, RecoveryKey)` 保存までとする。
- 複数デバイス管理UI、デバイス一覧、デバイス失効UI。
- Organization共有、sealed boxによるOrg DEK配布、Orgメンバー管理、Org DEKローテーション。
- AWS/ECR/Lambda/Neon本番デプロイ、クレデンシャル投入、WAF/API Gateway/CloudFront設定。
- `docs/01_企画書.md` / `docs/02_機能仕様書.md` / `docs/03_技術仕様書.md` / `docs/05_設計判断記録.md` の変更。
- git commit。

## 5. 実装手順（例）

1. 指定ファイルを読み、`docs/03` §4.3の各鍵の保存先、§4.8のDEK対応、§7.2/§7.3の登録/ログイン順序をメモする。
2. サーバーmigrationの形を先に決める。ユーザー単位鍵、テナント単位鍵、リスト単位鍵を混同しない。個人リストのList DEK wrapはlist_id単位で取得できる必要がある。
3. `core/crypto` に鍵生成・KEK導出・wrap/unwrap・Recovery Key生成の単体テストを追加する。roundtrip、誤鍵失敗、AAD/context違い失敗、zeroize対象型の破棄をテストまたはコードレビュー項目で固定する。
4. `core/sync` のaccount clientでOPAQUE登録/ログインを実装する。task-01のPoCとtask-70のHTTP DTOを照合し、exportKeyはクライアント内部だけで使う。
5. サーバーのregister/login finish DTOを拡張し、testcontainers上でラップ済み鍵の保存/返却を検証する。
6. Keychain storeを汎用化する。service名は少なくとも `com.taskveil.app.device-key`、`com.taskveil.app.session-token`、`com.taskveil.app.master-key-wrap` 相当で分離する。命名はコード内定数化し、秘密値をログに出さない。
7. `app/rust/src/api.rs` へFRB APIを追加する。`AccountSessionStateDto` のようなDTOはメール、user_id、tenant_id、device_id、logged_in程度に留め、鍵バイト列やセッショントークン平文を返さない。Recovery Keyだけは登録直後の一度表示用文字列として返してよい。
8. FRB生成を実行し、Dart bridge/fakeへメソッドを追加する。
9. Flutterのaccount providerと画面を追加する。エラー表示は安全な汎用文言にし、サーバーからの内部エラーや秘密情報をそのまま表示しない。
10. Lists画面の右上にoverflow menuを追加し、「アカウント」から `/account` へ遷移させる。icon-only controlはtooltip/semanticsを付ける。
11. Rust統合テストとFlutter widget testを追加し、品質ゲートを実行する。ローカルport bindやDockerが環境制約で失敗した場合は、実装とテストは残し、完了報告へ環境起因として明記する。
12. 本ファイルへ `## 9. 完了報告` を追記し、README/BACKLOGを完了状態へ更新する。

## 6. 受け入れ基準

- [ ] `docs/tasks/README.md` の共通受け入れ基準を満たすこと。

タスク固有の受け入れ基準:

- [ ] `core/crypto` にMK/KEK_pw/Recovery Key/User X25519鍵/List DEK/Tenant Root DEKの生成・wrap/unwrap helperがあり、roundtrip、誤鍵失敗、AAD/context違い失敗の単体テストがある。
- [ ] `core/sync` または共有Rust crateに `opaque-ke 3.0.0` クライアントフローと `reqwest(rustls)` account clientがあり、Dart側HTTP実装なしで登録/ログインできる。
- [ ] サーバーmigration/APIが `wrap(MK, KEK_pw)`、`wrap(MK, RecoveryKey)`、User X25519公開鍵、`wrap(SK, MK)`、個人Tenant Root DEK wrap、個人List DEK wrapを保存/返却でき、平文鍵を受け取らない。
- [ ] 登録時にMK、Recovery Key、User X25519鍵ペア、個人Tenant Root DEK、既存個人リスト用List DEKが生成され、`docs/03` §4.3/§4.8どおりの保存先にラップ済みで配置される。
- [ ] ログイン時にOPAQUE exportKey由来KEKで同一MKを復元でき、User X25519秘密鍵、Tenant Root DEK、List DEKのunwrapまでテストされている。
- [ ] 新デバイスログイン時にserver `devices` 行が作成され、`wrap(MK, DK)` はサーバーへ送られずローカルKeychainへ保存される。
- [ ] Keychain service名がDevice Key、session token、local wrapped MKで分離され、ログアウト時にsession tokenとメモリ上の鍵状態が破棄される。
- [ ] FRBのregister/login/logout/session state APIが追加され、MK/KEK/DEK/DK/exportKey/セッショントークン平文をDart境界へ返さない。Recovery Keyは登録直後の一度表示だけに限定される。
- [ ] FlutterにLists画面overflowの「アカウント」導線、`/account` route、未ログイン/ログイン済み/ログアウト/サーバーURL設定の最小UIがあり、UI文字列はen/ja ARB化されている。
- [ ] testcontainers Postgres + 実axum server + Rust account clientで、登録→ログアウト→ログイン→MK復元、誤パスワード、失効セッション、誤鍵unwrap失敗の統合テストが成功している。
- [ ] Flutter widget testがfake bridgeで、未ログイン表示、登録成功後のRecovery Key一度表示、ログイン済みメール表示、ログアウト復帰、サーバーURL設定を確認している。
- [ ] 秘密情報ログ禁止の確認結果（対象: password、session token、MK、KEK、DEK、DK、exportKey、Recovery Key平文、OPAQUE state bytes）を完了報告に記録している。

## 7. 制約・注意事項

- `docs/03_技術仕様書.md` §4を優先する。平文MKは永続保存しない。Keychainに保存するローカルMK材料は `wrap(MK, DK)` であり、平文MKではない。
- SQLCipherのローカルDB鍵は従来どおりDK由来の `taskveil/local-db-key/v1` から導出する。アカウント登録時にPRAGMA rekeyしない。
- exportKeyをMKとして使わない。exportKeyはKEK_pw導出だけに使い、ログイン処理後にzeroizeする。
- MKは不変である。パスワード変更は本タスク外だが、将来のために `wrap(MK, KEK_pw)` を差し替えられるデータ配置にする。
- List DEKとTenant Root DEKを混同しない。tasks/comments/remindersはList DEK、tags/templates/schedules/timer_sessionsはTenant Root DEKである。
- サーバーは暗号blobや鍵の平文を解釈しない。ラップ済み鍵と公開鍵、device metadata、session metadataだけを扱う。
- `devices.public_key` は将来のデバイス単位鍵配布用予約フィールドである。Org共有のUser X25519鍵ペアとは役割が違うため、混同しない。
- Recovery Key平文はサーバーへ送らない。端末にも保存しない。登録完了画面で一度だけ表示し、完了後は再表示しない。
- セッショントークンは認証用であり、暗号鍵やHKDF入力に使わない。
- Flutter/Dartのフォーム入力としてパスワードは一時的にDartを通るが、鍵バイト列はDartへ出さない。Rust側で扱う秘密byte/stringは可能な限り `zeroize` 対応型で保持する。
- `flutter_rust_bridge_codegen` は `app/rust/src/api.rs` 変更後に必ず実行する。生成物は手編集しない。
- testcontainersはDocker daemonが動いている前提でよい。ローカルsocket bindがsandboxで禁止される場合は、テスト実行不能の理由を完了報告に明記する。

## 8. 完了報告に含めるべき内容

- 作業日、読んだファイル
- 追加/変更したRust依存crateと用途
- 鍵階層実装の要約（MK、KEK_pw、Recovery Key、User X25519鍵、Tenant Root DEK、List DEK、各wrapの保存先）
- 追加したserver migrationと、ラップ済み鍵/公開鍵/device/sessionの保存先
- 登録フローの詳細（OPAQUE client、exportKey、MK生成、Recovery Key一度表示、server payload、local Keychain保存）
- ログイン/新デバイスフローの詳細（MK復元、User秘密鍵/DEK unwrap、device登録、`wrap(MK, DK)` local保存）
- logout/session stateの挙動（server revoke、Keychain削除対象、メモリ鍵破棄、ローカルDB/Device Keyを消さないこと）
- FRB公開API一覧とDTO概要（Dart境界へ返さない秘密値を明記）
- Flutter UI変更、追加route、追加ARB key、widget test名
- testcontainers + 実サーバーRust統合テストの対象と実行結果
- zeroize適用箇所と秘密情報ログ禁止の確認結果
- FRB再生成の実行結果
- 品質ゲート実行結果
- 変更ファイル一覧
- 未解決事項（同期ループ、Recovery UX完全版、複数デバイス管理UI、Org共有、実デプロイ等。無い場合も「なし」と明記）

## 9. 完了報告

### 作業日

- 2026-07-08

### 読んだファイル

- `AGENTS.md`
- `docs/tasks/README.md`
- `docs/tasks/BACKLOG.md`
- `docs/tasks/task-71-key-hierarchy-account.md`
- `core/crypto/src/key_hierarchy.rs`
- `core/sync/src/account.rs`
- `server/migrations/202607080002_account_key_bundles.sql`
- `server/src/auth.rs`
- `server/src/db.rs`
- `server/src/routes/auth.rs`
- `server/tests/sync_server.rs`
- `app/rust/src/api.rs`
- `app/rust/src/dev_key_store.rs`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/router.dart`
- `app/lib/src/screens/lists_screen.dart`
- `app/lib/src/screens/account_screen.dart`
- `app/test/account_screen_test.dart`
- `app/test/visual_qa/visual_qa_screenshots_test.dart`
- `app/tool/visual_qa.sh`

### 追加/変更したRust依存crate

- `reqwest`（`rustls-tls` + `json`）: `core/sync` の account HTTP client。
- `zeroize`: パスワード、OPAQUE exportKey、MK/DEK/Recovery Key等のメモリ破棄。
- `x25519-dalek`: User X25519鍵ペアとデバイス公開鍵生成。
- `core/sync` で既存workspace依存の `base64` / `chrono` / `opaque-ke` / `rand` / `uuid` を参照。
- `app/rust` で `taskveil-sync` / `tokio` / `zeroize` を参照。

### 鍵階層実装

- `core/crypto/src/key_hierarchy.rs` を追加し、MK、Tenant Root DEK、List DEKを32byte乱数で生成するhelperを追加した。
- OPAQUE exportKeyから `info=taskveil/kek-pw/v1` でKEK_pwを導出する。
- Recovery Keyは24語文字列として生成し、`info=taskveil/recovery-key-wrap-key/v1` でwrap鍵を導出する。
- User X25519鍵ペアを生成し、公開鍵のみサーバー保存対象にする。
- wrap/unwrapは既存AEADを使い、用途別AADを以下に分離した。
  - `taskveil/wrap/mk-by-kek-pw/v1`
  - `taskveil/wrap/mk-by-device-key/v1`
  - `taskveil/wrap/mk-by-recovery-key/v1`
  - `taskveil/wrap/user-x25519-sk-by-mk/v1`
  - `taskveil/wrap/tenant-root-dek-by-mk/v1`
  - `taskveil/wrap/list-dek-by-mk/v1`
- `core/crypto` 単体テストで、鍵長、KEK_pw context固定、Recovery Key導出、password/DK/Recovery wrap roundtrip、誤鍵失敗、AAD/context違い失敗、User秘密鍵/Tenant Root DEK/List DEK roundtripを確認した。

### server migration/API

- `server/migrations/202607080002_account_key_bundles.sql` を追加した。
- `user_key_bundles` に `wrap(MK, KEK_pw)`、`wrap(MK, RecoveryKey)`、User X25519公開鍵、`wrap(User SK, MK)` を保存する。
- `tenant_key_bundles` に個人Tenant Root DEK wrapを保存する。
- `list_key_bundles` に個人List DEK wrapを `tenant_id, list_id` 単位で保存する。
- `devices.public_key` にログイン/登録デバイスの公開鍵を保存する。
- `/v1/auth/register/finish` はOPAQUE finish messageとラップ済みkey bundle、device public keyを受け取る。
- `/v1/auth/login/finish` はセッションと復号用key bundleを返す。
- `/v1/auth/logout` を追加し、Bearer tokenに対応するsessionを失効する。

### 登録フロー

- `core/sync/src/account.rs` の登録処理は、メール要求、OTP確認、OPAQUE start、
  finish、status reconciliationを個別のdurable段階として公開する。
- client側でOPAQUE register start/finishを実行し、exportKeyからKEK_pwを導出する。
- 登録時にMK、Recovery Key、User X25519鍵ペア、Tenant Root DEK、個人List DEKを生成する。
- サーバーpayloadにはラップ済みkey bundleと公開鍵のみを含める。パスワード、exportKey、MK、KEK、DEK、Recovery Key平文は送らない。
- ローカルにはDevice Keyで `wrap(MK, DK)` を作成し、Keychain/テスト時file fallbackへ保存する。
- FRBの `accountRegistrationComplete` は登録直後だけ `recoveryKey` をDartへ返す。

### ログイン/新デバイスフロー

- `core/sync/src/account.rs` の `AccountClient::begin_login` から、
  durableなdevice certification再開フローを開始する。
- login finish responseの `wrap(MK, KEK_pw)` をOPAQUE exportKey由来KEK_pwで復号し、MKを復元する。
- MKでUser X25519秘密鍵、Tenant Root DEK、List DEKをunwrapする。
- ログインごとにserver `devices` 行を作成し、device public keyを保存する。
- `wrap(MK, DK)` はサーバーへ送らず、ローカル保存だけに使う。

### logout/session state

- `accountLogout` は保存済みsession tokenがある場合にserver `/v1/auth/logout` を呼び、ローカルのsession tokenとlocal wrapped MKを削除する。
- logout時はアカウント設定値とメモリ上の `AccountRuntimeState` を破棄する。
- logoutではローカルDB、Device Key、Todoデータは削除しない。
- `getAccountSessionState` はsession tokenとlocal wrapped MK、account metadataが揃う場合だけ `loggedIn=true` を返す。

### Keychain/ローカル保存

- `app/rust/src/dev_key_store.rs` でaccount secret用storeを追加した。
- Device Key、session token、local wrapped MKのservice名を以下に分離した。
  - `com.taskveil.app.device-key`
  - `com.taskveil.app.session-token`
  - `com.taskveil.app.master-key-wrap`
- Flutter test processでは実Keychainに触れず、file fallbackを使う。

### FRB公開API/DTO

- 追加API:
  - `accountRegistrationBegin(email, serverUrl?)`
  - `accountRegistrationVerifyOtp(otp)`
  - `accountRegistrationComplete(password, deviceName?)`
  - `accountLogin(email, password, serverUrl?, deviceName?)`
  - `accountLogout()`
  - `getAccountSessionState()`
  - `getSyncServerUrl()`
  - `setSyncServerUrl(serverUrl)`
- `AccountSessionStateDto` は `loggedIn`、account ID群に加えて、Recovery Key表示を
  通常画面より先に再開するための `recoveryPending` を返す。
- `AccountAuthResultDto` は `session` と登録直後だけの `recoveryKey` を返す。
- 未確認のRecovery KeyはOS保護credential内の`RecoveryDisplayPending`として保持し、
  明示ack前のlogoutや秘密の破棄を拒否する。
- MK/KEK/DEK/DK/exportKey/session token平文はDart境界へ返さない。
- `flutter_rust_bridge_codegen generate --config-file flutter_rust_bridge.yaml` 済みの生成物が差分に含まれる。

### Flutter UI/test

- `/account` routeと `AccountScreen` を追加した。
- Lists画面右上overflowに「Account」導線を追加した。
- Account画面にサーバーURL設定、ログイン/登録フォーム、登録直後Recovery Key表示、ログイン済みメール表示、logoutボタンを追加した。
- サーバーURL設定キーは `sync_server_url`、既定値は `http://localhost:3000`。
- UI文字列は `app/lib/l10n/app_en.arb` / `app/lib/l10n/app_ja.arb` に追加し、`app/lib/src/generated/l10n/` を生成更新した。
- `app/test/account_screen_test.dart` で未ログイン画面、登録成功後Recovery Key一度表示、ログイン済みメール表示、logout復帰、サーバーURL保存を確認した。
- visual QAに `account_signed_out` を追加し、`app/build/visual_qa/account_signed_out.png` を生成した。
- 事前退避先: `app/build/visual_qa_before_task71_20260708100102`。

### Rust統合テスト内訳

- `server/tests/sync_server.rs` の全4件が成功した。
  - `migration_creates_sync_server_schema_and_health_works`
  - `opaque_registration_login_reuse_expiry_and_cleanup_are_enforced`
  - `push_pull_seq_invariants_tenant_isolation_and_revoked_devices_are_enforced`
  - `account_client_register_logout_login_restores_keys_and_rejects_invalid_states`
- task-71向け統合テストでは、testcontainers Postgres + 実axum server + Rust `AccountClient` で、登録、Recovery Key生成、server保存bundle取得、誤鍵unwrap失敗、logout後の失効session拒否、再login、同一MK/User SK/Tenant Root DEK/List DEK復元、別Device Keyでlocal wrapped MKが変わること、誤パスワード拒否、2件のdevice登録を確認した。

### zeroize/秘密情報ログ禁止

- `core/sync/src/account.rs` でpassword、OPAQUE exportKey、KEK_pw、Recovery wrap key、Recovery Key、MK/User SK/Tenant Root DEK/List DEKを `Zeroizing` または明示 `zeroize()` で扱う。
- `app/rust/src/api.rs` でDartから受け取ったpasswordを認証処理後に `zeroize()` する。
- `git grep` / `grep -R` で `println!`、`dbg!`、`eprintln!`、`tracing::`、`log::`、`debugPrint`、`print(`、`developer.log`、`Logger` を確認した。
- task-71変更範囲に、password、session token、MK、KEK、DEK、DK、exportKey、Recovery Key平文、OPAQUE state bytesをログ出力する箇所は見つからなかった。
- 既存の出力箇所として、Keychain fallback文言、server起動/shutdown/sqlx error、Flutter init error、性能test計測ログ、cargokit内部ログがある。task-71の秘密値を出力するものではない。

### 品質ゲート実行結果

- `cargo fmt --all -- --check`: 成功
- `cargo clippy --workspace -- -D warnings`: 成功
- `cargo test --workspace`: 成功
  - `taskveil_crypto`: 24 passed
  - `taskveil_domain`: 40 passed
  - `taskveil_server` unit/main: 0 passed
  - `server/tests/sync_server.rs`: 4 passed
  - `taskveil_storage`: 48 passed, 1 ignored
  - `taskveil_sync`: 27 passed
  - `taskveil_app_bridge`: 4 passed, 1 ignored
  - doc tests: 成功
- `cd app && flutter analyze`: 成功
- `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release`: 成功
- `cd app && flutter test`: 成功（120 passed, visual QA harness 1 skipped）
- `sh app/tool/check_hardcoded_strings.sh`: 成功
- `sh app/tool/visual_qa.sh`: 成功（42 screenshots/tests passed）
- `git diff --check`: 成功

### 変更ファイル一覧

- `Cargo.lock`
- `Cargo.toml`
- `core/crypto/Cargo.toml`
- `core/crypto/src/lib.rs`
- `core/crypto/src/key_hierarchy.rs`
- `core/sync/Cargo.toml`
- `core/sync/src/lib.rs`
- `core/sync/src/account.rs`
- `server/migrations/202607080002_account_key_bundles.sql`
- `server/src/auth.rs`
- `server/src/db.rs`
- `server/src/routes/auth.rs`
- `server/tests/sync_server.rs`
- `app/rust/Cargo.toml`
- `app/rust/src/api.rs`
- `app/rust/src/dev_key_store.rs`
- `app/rust/src/frb_generated.rs`
- `app/lib/l10n/app_en.arb`
- `app/lib/l10n/app_ja.arb`
- `app/lib/src/core/bridge_service.dart`
- `app/lib/src/core/providers.dart`
- `app/lib/src/generated/l10n/app_localizations.dart`
- `app/lib/src/generated/l10n/app_localizations_en.dart`
- `app/lib/src/generated/l10n/app_localizations_ja.dart`
- `app/lib/src/router.dart`
- `app/lib/src/rust/api.dart`
- `app/lib/src/rust/frb_generated.dart`
- `app/lib/src/rust/frb_generated.io.dart`
- `app/lib/src/screens/lists_screen.dart`
- `app/lib/src/screens/account_screen.dart`
- `app/test/support/fake_bridge_service.dart`
- `app/test/account_screen_test.dart`
- `app/test/visual_qa/visual_qa_screenshots_test.dart`
- `docs/tasks/README.md`
- `docs/tasks/BACKLOG.md`
- `docs/tasks/task-71-key-hierarchy-account.md`

### 未解決事項

- P2-M4の同期ループ、push/pull自動実行、outbox ACK、pull復号マージ、Flutter invalidate連携は未実装。
- Recovery Keyによるパスワード復旧UX完全版は未実装。
- 複数デバイス管理UI、デバイス一覧、デバイス失効UIは未実装。
- Organization共有、sealed boxによるOrg DEK配布、Orgメンバー管理、Org DEKローテーションは未実装。
- AWS/ECR/Lambda/Neon本番デプロイ、クレデンシャル投入、WAF/API Gateway/CloudFront設定は未実施。

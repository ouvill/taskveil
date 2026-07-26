# Phase 2計画書

E2EE Todoアプリ「Taskveil」のPhase 2を、実装可能なマイルストーンと検証可能な完了条件に分解する。Phase 2はアカウント機能、OPAQUE認証、E2EE同期、マルチデバイス利用、カレンダー表示、タイマー/Pomodoro、テンプレート・繰り返しタスクを扱う。サーバーはE2EEデータの中身を解釈せず、暗号blobと最小限の同期メタデータのみを扱う（`docs/03_技術仕様書.md` §4、§6）。

背景: 2026-07-08にプロダクトオーナーがPhase 2の自律実装を承認した。本計画はその実装範囲と完了条件を定義する。初版は同期系に絞っていたが、同日に `docs/01_企画書.md` §8と整合するようカレンダー表示、タイマー/Pomodoro、テンプレート・繰り返しタスクを追加した。2026-07-18現在、P2-M1〜M4・M6〜M8は完了した。P2-M5は削除同期、macOS / iOS Simulator確認、Android Rust FFI / Flutter release APK build、Android Keystore実装に加え、Pixel 7a / Android 16接続実機でのKeystore / Device Key rotation / DB reopenまで完了し、Android実機の同期確認が残る。一般リリースは別work itemの課金基盤を完成させるまで延期する。

本書のマイルストーン表は実装契約と完了条件の履歴であり、現在のwork item状態は `docs/tasks/work-*.md` のfront matterを正本とする。

## 1. Phase 2スコープ

`docs/01_企画書.md` §8のPhase 2記述のうち、本計画ではE2EE同期、アカウント、OPAQUE認証、マルチデバイス、カレンダー表示、Pomodoro/通常タイマー、テンプレート・繰り返しタスクを扱う。課金はApp Store IAP、server-side検証、非公開事業判断と不可分であるため、人間協働必須の別work itemとし、最初の一般リリースを解放するrelease gateに位置づける。

| 分類 | Phase 2に含める機能 | 判断 |
|---|---|---|
| アカウント | OPAQUE登録/ログイン、ログアウト、最小アカウント画面、セッション管理 | E2EE同期の入口であり、`docs/03_技術仕様書.md` §1.5、§7に直結するため |
| 鍵階層 | MK生成、exportKey由来KEKでのMKラップ、DKでのローカルMKラップ、DEK、デバイス登録 | コンテンツE2EEとマルチデバイス復号の前提であり、`docs/03_技術仕様書.md` §4を実装へ落とすため |
| クライアント同期 | HLC、フィールドHLCマップ、フィールドレベルLWW、outbox、pullカーソル、再push規約 | `docs/03_技術仕様書.md` §6.3、§6.4、ADR-004/005の中核 |
| サーバー | Postgresスキーマ、OPAQUE中間状態、push/pull API、tenant_seq採番、認可、同期不変条件 | `docs/03_技術仕様書.md` §1.5、§6.1、§6.2、§6.6、ADR-003/005/008に準拠 |
| マルチデバイス | 新デバイスログイン、初回フル同期、ローカルDB構築、端末間収束 | `docs/03_技術仕様書.md` §7.3の完了条件 |
| 削除同期 | ADR-016のarchive-first、bounded tombstone、server-trusted continuity、expired-device rebase | task-97 / task-98で裁定・実装済み |
| カレンダー表示 | 月表示・週表示、due/scheduledのプロット、ドラッグによる日付変更 | `docs/02_機能仕様書.md` F-13をPhase 2後半で実装するため |
| タイマー/Pomodoro | Pomodoro、通常作業タイマー、見積vs実績の記録面 | `docs/02_機能仕様書.md` F-16〜F-18と `docs/design/visual-direction.md` Focus Timer方針を実装へ落とすため |
| テンプレート・繰り返し | タスクテンプレート、RRULE準拠の繰り返し生成、streakの最小記録 | `docs/02_機能仕様書.md` F-19〜F-21をPhase 2後半で扱うため |

## 2. スコープ外

| 項目 | 判断 |
|---|---|
| ストア提出、リリースビルド署名、public release作業 | 人間のDeveloper Program、証明書、ストア判断が必要なため本計画外 |
| 実AWS/ECR/Lambda/Neon本番デプロイ | クレデンシャルと前段構成の人間判断が必要。課金release gateと同じリリース準備列で扱う |
| 課金、IAP、外部課金、レシート検証 | 人間協働必須のBilling foundation release gate work itemで扱う。完了するまで一般リリースしない |
| 法務、規約、プライバシーポリシー、監査依頼 | `docs/legal_overview.md` の公開方針は参照するが、判断は人間が行う |
| Organization共有、コメント、メンバー管理、Org DEKローテーション | Phase 3以降。Phase 2では個人テナントの同期に閉じる |
| タグUI、統計、高機能UI、CLI/MCP実機能 | Phase 3以降 |
| プッシュ通知 | E2EE設計上リマインダーはローカル通知が正（`docs/03_技術仕様書.md` §4.10）。サーバーpush通知は本計画外 |

検索UIは当初本計画のスコープ外だったが、task-103で既存FTS5をproduction UIへ先行接続し、独立検証まで完了した。

## 3. 現状と前提

| 領域 | 現状 | Phase 2への影響 |
|---|---|---|
| `core/sync` | protocol v5、HLC / field LWW、outbox、full resync、continuity、timer recordまで実装済み | Canonical Inbox convergenceがmaintenance backlog |
| `server/` | OPAQUE認証、Postgres同期、RLS、continuity / resync route、RevenueCat billing event / entitlementを実装済み | 外部store / provider設定とsandbox E2Eを課金release gateで完了する |
| OPAQUE / account | 登録・ログイン・session復元・MK / DEK接続を実装済み | 本番デプロイと実端末の最終確認が残る |
| ローカルDB | SQLCipher schema v18。同期、期限、計画属性、timerまでmigration済み | 課金状態をlocal authorityにせず、必要なcacheだけを後続設計する |
| 削除モデル | ADR-016とtask-98でarchive-first、terminal tombstone、expired-device rebaseを実装済み | 実本番環境での運用観測が残る |

## 4. マイルストーン

### P2-M1: クライアント同期基盤

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M1-01 | `core/sync` のHLCを仕様準拠へ拡張する。固定幅ソート可能エンコード、decode、受信HLCとのmerge、未来HLC検出用の比較補助を実装する | 既存 `core/sync` 雛形 | HLCの順序、一意性、encode/decode roundtrip、時計後退時の単調性テストが通ること（`docs/03_技術仕様書.md` §6.3） |
| P2-M1-02 | フィールドHLCマップとフィールドレベルLWWマージを実装する | P2-M1-01 | 同一レコードの異なるフィールド同時編集が両方残る単体テスト、同一フィールド競合で後勝ちになる単体テストが通ること（§6.3、§6.5、ADR-004/005） |
| P2-M1-03 | 同期対象レコードのblob暗号エンベロープを定義する。DEK、XChaCha20-Poly1305、AAD（record_id/collection）、`{fields, field_hlcs}` 内包を扱う | P2-M1-02、`core/crypto` | 改ざんAAD失敗、誤DEK失敗、record_id/collection入れ替え失敗、正常roundtripのテストが通ること（§4.8） |
| P2-M1-04 | `core/storage` にoutbox、sync cursor、必要なローカル同期メタデータのmigrationを追加する | P2-M1-02 | 既存DBからのmigration、ACK前outbox保持、ACK後削除、pull cursor前進のrepositoryテストが通ること（§6.4） |
| P2-M1-05 | 収束性のproperty-based testingを追加する | P2-M1-01〜04 | `proptest` 等で任意の編集順・pull順・再push順に対して全端末が同一状態へ収束するテストが通ること（§11.1） |

### P2-M2: サーバー実装

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M2-01 | Postgresアクセス層とmigrationを整備する。`users`、`devices`、個人tenant、ラップ済み鍵、OPAQUE ephemeral、`sync_records`、`tenant_seq`、`sync_records_history` を定義する | なし。P2-M1と並行可 | リポジトリ内で完結するPostgresテスト環境（embedded/testcontainers等。グローバルインストール回避）でmigrationと基本CRUDが通ること（§1.5、§6.2、ADR-003/008） |
| P2-M2-02 | OPAQUE登録/ログインAPIを実装する。中間状態はPostgres ephemeral tableに保存し、consume時に削除する | P2-M2-01、task-01 PoC | 登録、ログイン、exportKey互換、期限切れ/再利用不可、誤パスワード失敗のAPIテストが通ること（§1.5、§7.2、§7.3） |
| P2-M2-03 | セッションとデバイス認可を実装する | P2-M2-02 | 失効デバイスがpush/pull不可、認可されていないtenantへアクセス不可、リクエストごとの検証テストが通ること（§6.1、§7.6） |
| P2-M2-04 | push APIを実装する。tenant_seq行ロックによるseq採番、HLC比較、history退避、冪等no-opを扱う | P2-M2-01、P2-M2-03、P2-M1のデータ形状 | 採用/superseded/no-opの応答、seq可視順、並行push直列化、history 30日保持前提の退避テストが通ること（§6.2、§6.4、ADR-005） |
| P2-M2-05 | pull APIを実装する。`since`、`limit`、`next_since`、`has_more`、初回 `since=0` を扱う | P2-M2-04 | ページング、エコー除外なし、cursor前進、tenant分離、limit上限のAPIテストが通ること（§6.4） |
| P2-M2-06 | §6.6のサーバー不変条件を強制する | P2-M2-04〜05 | blobサイズ上限、batch上限、サーバー時刻+5分境界以上の未来HLC拒否、物理DELETE不可方針、認可/課金フックのテストが通ること（§6.6） |

### P2-M3: 鍵階層とアカウント接続

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M3-01 | クライアント側でMK生成、exportKey由来KEK、`wrap(MK, KEK_pw)`、`wrap(MK, DK)`、Recovery Key用wrapの最小実装を行う | P2-M2-02 | パスワード平文・MK・DK・exportKeyをログ出力しないテスト/レビュー項目を含め、wrap/unwrap roundtripと誤鍵失敗が通ること（§4.2〜§4.5） |
| P2-M3-02 | 個人テナント用Tenant Root DEKとList DEKを導入し、同期blob生成時の鍵選択を接続する | P2-M1-03、P2-M3-01 | lists/tasksの対応DEKが仕様どおりで、DEK誤用時に復号できないテストが通ること（§4.2、§4.8、ADR-007） |
| P2-M3-03 | デバイス登録と新デバイスログインを接続する | P2-M2-03、P2-M3-01 | 新デバイスでOPAQUEログイン後にMKを復号し、`wrap(MK, DK)` をローカル保存し、`since=0` 初回同期へ進めること（§7.3） |
| P2-M3-04 | Flutterに最小アカウント画面を追加する。登録、ログイン、ログアウト、現在の同期状態表示を扱う | P2-M3-01〜03 | en/ja ARB、widget test、失敗時の安全なエラー表示、ログアウト後のセッション破棄が確認できること（§7） |
| P2-M3-05 | セッション管理とローカルアカウント状態を永続化する | P2-M2-03、P2-M3-04 | アプリ再起動後のログイン状態復元、ログアウト時のセッション削除、失効時の再ログイン要求がテストで確認できること |

### P2-M4: 同期エンジン統合

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M4-01 | クライアント同期ループを実装する。push、pull、ACK、cursor前進、指数バックオフ、フォアグラウンド復帰時pullを扱う | P2-M1、P2-M2、P2-M3 | ローカルテストサーバーまたはテストダブルで、オフライン中のoutbox蓄積と復帰後同期が通ること（§6.4） |
| P2-M4-02 | pull復号、フィールドLWWマージ、ローカルDB反映、UI invalidateを接続する | P2-M4-01 | 他端末更新がFlutter画面へ反映され、同一フィールド競合/異フィールド競合のwidgetまたはintegration testが通ること（§6.4、§6.5） |
| P2-M4-03 | 再push規約を実装する | P2-M4-02 | pullしたblobよりローカルが勝つフィールドがある場合、マージ済みblobが再pushされ、最終的に両端末が一致するテストが通ること（§6.4、ADR-005） |
| P2-M4-04 | オフライン耐性とエラー状態をUIへ反映する | P2-M4-01〜03 | ネットワーク失敗、認証失効、サーバー拒否、再試行中の表示が直書き文字列なしで確認できること |

### P2-M5: 削除同期とマルチプラットフォーム検証

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M5-01 | Archive-first削除同期を裁定する | P2-M4 | ADR-016がAcceptedとなり、bounded tombstone、server-trusted continuity、expired-device rebase、late descendant収束が決定していること（完了） |
| P2-M5-02 | ADR-016の削除同期を実装する | P2-M5-01 | terminal deletion、history purge、pull-before-push、continuity、expired rebase、late descendant cascadeが自動テストで確認できること（task-98で完了） |
| P2-M5-03 | Android build、macOS / iOS動作を検証する | P2-M4、P2-M5-02 | macOSとiOS Simulatorの主要動線、Android Rust FFI / Flutter build、Android Keystore、Android同期が確認され、既知差分が記録されること（Android Rust FFI / release APK・AAB build / Keystore実装、Pixel 7a / Android 16接続実機のKeystore / Device Key rotation / DB reopenは完了。Android Emulator上の通知・主要UI・登録・ログイン・2 profile双方向同期を自動化し、接続実機での最終確認はrelease gateとして維持する） |
| P2-M5-04 | Phase 2完了前の品質ゲートを通す | P2-M5-03 | Rust/Flutterの品質ゲート、ハードコード文字列検出、主要同期テスト、Postgres統合テストが通ること |

### P2-M6: カレンダー表示

`docs/02_機能仕様書.md` F-13に従い、締切日時（due）と作業予定日時（scheduled）を月/週カレンダーへ表示し、日付変更をタスク編集へ反映する。サーバー同期済み環境でも、カレンダー操作は通常のローカルタスク更新としてoutboxに入り、既存同期エンジンで伝播する。

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M6-01 | カレンダー画面の情報設計を作る。月表示/週表示、Todayへの戻り、due/scheduledの区別、過密日の表示上限を決める | P2-M4 | 既存Home/List/Detailと矛盾しないルート、ナビゲーション、空/過密/エラー状態が設計されていること |
| P2-M6-02 | カレンダー用の集約APIを追加する。指定期間内のdue/scheduledタスクを取得し、タイムゾーンと終日扱いを明示する | P2-M6-01 | dueのみ、scheduledのみ、両方あり、期限超過、タイムゾーン境界のRust/Flutterテストが通ること |
| P2-M6-03 | 月表示と週表示UIを実装する | P2-M6-02 | 月/週の切り替え、日付選択、タスク詳細への遷移、長いタイトルの折り返し、en/ja ARBが確認できること |
| P2-M6-04 | カレンダー上のドラッグまたは代替操作で日付変更を実装する | P2-M6-03 | ドラッグが使える環境では日付変更でき、モバイル/アクセシビリティ向け代替操作でも同じ更新ができること |
| P2-M6-05 | 同期済み2端末でカレンダー更新の収束を確認する | P2-M6-04 | 片端末で日付変更したタスクが他端末のカレンダーへ反映され、競合時は既存LWW規約に従うこと |

### P2-M7: タイマー/Pomodoro

`docs/02_機能仕様書.md` F-16〜F-18に従い、Pomodoro、通常作業タイマー、見積vs実績比較の土台を実装する。デザインソースは `docs/design/visual-direction.md` のFocus Timer節と、task-22 Design Labのfocus timerモックである。タイマーは「今これをする」という宣言面であり、streak、ランキング、トロフィー、罪悪感を煽る文言、常設の大きなスコアボードは採用しない。

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M7-01 | タイマー状態モデルを設計する。通常タイマー、Pomodoro、open-ended focus、開始/一時停止/完了/中断、タスク紐付け、バックグラウンド復帰を明示する | P2-M4 | 状態遷移表と永続化方針があり、アプリ再起動時の復元/破棄判断がテスト可能になっていること |
| P2-M7-02 | `timer_sessions` のローカル永続化と同期対象plaintextを実装する | P2-M7-01 | タスクid、開始/終了、実績秒数、モード、Pomodoro設定、見積比較に必要な値が保存され、E2EE blobとして同期できること |
| P2-M7-03 | Pomodoroタイマーを実装する。初期値は作業25分/休憩5分、長休憩までのセッション間隔を設定可能にする | P2-M7-02 | 作業/休憩/長休憩、設定変更、タスク紐付け、完了記録のテストが通ること |
| P2-M7-04 | 通常作業タイマーを実装する | P2-M7-02 | 開始/停止のみのストップウォッチ型計測ができ、計測時間がタスクの作業実績として残ること |
| P2-M7-05 | 見積vs実績の最小表示を追加する | P2-M7-03〜04 | タスクの見積所要時間とtimer_sessionsの実績時間を比較でき、将来の統計画面F-22へ接続可能な集計APIがあること |
| P2-M7-06 | Focus Timer UIを実装する | P2-M7-03〜05 | 選択タスクを中心に、start/pause、finish、add time、exitがモバイルで押しやすく、直書き文字列なしで表示されること |

### P2-M8: テンプレート・繰り返しタスク

`docs/02_機能仕様書.md` F-19〜F-21に従い、テンプレート保存、RRULE準拠の繰り返し生成、習慣streakの最小記録を扱う。生成処理はローカル端末上で実行し、オフライン期間があっても次回起動時に未生成分を補完する。

| ID | 内容 | 依存タスク | 完了条件 |
|---|---|---|---|
| P2-M8-01 | テンプレートのデータモデルを設計する。タイトル、メモ、サブタスク構造、タグ等、インスタンス化時に引き継ぐ項目を明示する | P2-M4 | 通常タスクとの差分、同期対象、削除/更新時の扱いがテスト可能に整理されていること |
| P2-M8-02 | テンプレート保存とワンタップ起票を実装する | P2-M8-01 | 既存タスクからテンプレート保存でき、テンプレートから同等構造のタスクを作成できること |
| P2-M8-03 | RRULE準拠の繰り返しルールを実装する | P2-M8-01 | 毎日、毎週特定曜日、毎月特定日、カスタムルール、終了条件、タイムゾーン境界のテストが通ること |
| P2-M8-04 | 繰り返しタスクの自動生成を実装する | P2-M8-03 | アプリ起動時/復帰時に未生成分を補完し、重複生成せず、オフライン期間後も必要分が生成されること |
| P2-M8-05 | streakの最小記録と表示を実装する | P2-M8-04 | 繰り返しタスクの連続達成状況が記録される。ただし圧迫感のある演出やランキングを避け、静かな補助情報として扱うこと |
| P2-M8-06 | 同期済み2端末でテンプレート/繰り返し生成の整合を検証する | P2-M8-05 | テンプレート更新と生成済みインスタンスが端末間で収束し、同じ繰り返し予定が重複作成されないこと |

## 5. リスクと先送り判断

| リスク | 影響 | 対策または判断 |
|---|---|---|
| Canonical Inbox未収束 | 複数端末でdefault候補が分かれると、domain rowと認証済みplaintextの意味が一致しない | Accepted ADR-015を正本に、登録済みcritical work itemで決定的収束を実装する |
| Postgresテスト環境 | グローバルPostgres前提にすると実行エージェントやCIで再現不能になる | embedded/testcontainers等、リポジトリ内で完結する方式をP2-M2の完了条件にする |
| `opaque-ke` バージョン/API差分 | 認証互換性とexportKey導出が崩れる | Phase 1 PoCの `opaque-ke 3.0.0` を前提に固定し、更新は別判断にする |
| HLC実装の仕様逸脱 | 収束性、冪等性、未来HLC拒否が壊れる | 固定幅エンコード、受信merge、proptest、サーバー未来拒否テストを分けて検証する |
| E2EE blobのメタデータ漏洩 | サーバーが不要な平文情報を知る | record_id、collection、seq、HLC、blobサイズ以外をサーバー側メタデータに増やさない |
| Lambda/Neon本番デプロイ | クレデンシャル、課金、WAF/API Gateway判断が必要 | Docker / local Postgres検証と本番deployを分離し、課金release gate合格後のリリース列で実施する |
| 課金外部E2E未完 | iOS / Android購入client、server-side entitlement、失効認可のコードは完了したが、provider設定、App Store / Google Play商品設定、各store sandbox証跡が未完である | Billing foundation release gateを完了するまでstore提出、release tag、公開告知を行わない |
| ローカル通知体験 | 同期導入後も通知はサーバーpushではなくローカル通知である | iOS / Android接続実機で通知登録・取消・snoozeを最終確認し、同期サーバーから通知時刻を扱わない |
| カレンダーのドラッグ操作 | モバイル、デスクトップ、アクセシビリティで操作品質が割れる | ドラッグだけを唯一の操作にせず、詳細編集または日付変更メニューを同等経路として用意する |
| タイマーの圧迫感 | Pomodoroやstreakがスコア化されるとTaskveilの静かな体験から外れる | `docs/design/visual-direction.md` のFocus Timer方針を正とし、宣言的で可逆な操作、静かな記録に留める |
| 繰り返しタスクの重複生成 | 複数端末、オフライン復帰、時刻境界で同じインスタンスを重複作成する恐れがある | RRULE instance keyを定義し、ローカル生成と同期マージで冪等性をテストする |

## 6. Release gateと人間確認

- 課金provider、product、trial / grace、restore方針とiOS / Android client実装は揃っている。RevenueCat環境別mobile app、App Store / Google Play商品設定、各store sandbox E2Eを完了する。具体価格を含むstorefront設定は人間承認を正本とする。
- AWS/ECR/Lambda/Neon本番デプロイの実行、クレデンシャル投入、WAF/API GatewayまたはCloudFront前段の判断。
- iOS実機でKeychainゼロプロンプト、通知、購入・復元、同期を通し確認する。AndroidはEmulator自動E2Eに加えて、接続実機で通知、Google Play購入・復元、アカウント登録・ログイン、別端末同期を最終確認する。AndroidのKeystore / Device Key rotation / DB reopenはPixel 7a / Android 16で確認済みである。
- 課金、実機、本番運用、法務・コンプライアンスの各ゲート合格後にrelease branch / tag、ストア提出、公開告知を行う。

## 7. 既存仕様書との差異・確認事項

- `docs/01_企画書.md` §8のうち、同期、アカウント、カレンダー、Pomodoro、テンプレート / 繰り返し、Android Flutter release APK / AAB build、Android Keystore、通知permission / snooze、Google Play購入clientは実装済みである。Android Emulatorの主要UI・登録・ログイン・双方向同期を継続的に検証し、接続実機の通知・同期とApp Store / Google Play外部課金E2Eをrelease gateとして残す。
- 旧Turso route TODOはPostgres統合済みの`server/src/routes/auth.rs` / `sync.rs`へ置き換えられた。`server/src/routes/billing.rs`もRevenueCat adapter、webhook、billing GET / refresh、同期認可を実装済みで、外部sandbox E2EはactiveなBilling foundation work itemで追跡する。
- 削除同期の再設計待ちはADR-016とtask-98で解消済みであり、task-95のfull resync / GC horizon、task-96のRLS hardeningも完了した。

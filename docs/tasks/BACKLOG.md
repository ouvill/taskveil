# Taskveil バックログ

UUIDv7 work item方式のpilot中に残すlegacy backlogである。既存のLater、Quick fixes、Icebox、人間作業はpilot後の移行判断まで保持するが、新規候補や新形式work itemの状態はこのファイルへ追加しない。新形式の状態は各 `work-*.md` のfront matter、長期方向は [`docs/07_Phase1計画書.md`](../07_Phase1計画書.md) / [`docs/08_Phase2計画書.md`](../08_Phase2計画書.md) を正本とする。

## 運用ルール

- 完了項目をこのファイルへ残さない。
- pilot中は既存項目の全面移行を行わず、新しい候補を追加しない。
- 新規work itemに連番を採番しない。UUIDv7の生成と作成方法は [`PLAYBOOK.md`](./PLAYBOOK.md) に従う。
- Laterは最大20件を目安とする。上限を超えたら優先度を付けずIceboxへ移す。
- 仕様変更・依存追加・暗号・鍵・課金・public/private境界は、人間承認後にtaskへ昇格する。
- taskの結果はtask文書とgitへ残す。

## Later

| 候補 | 内容 | 出典・依存 |
|---|---|---|
| Windows / Linux本番Device Key Store | Windows current-user DPAPI、Linux Secret Serviceを実装し、平文 `device.key` / account secret fileを本番経路から除外する | ADR-011 / task-81。新規依存は人間承認が必要 |
| CLI実接続 | 共通client/profile層からFlutter desktopと同じSQLCipher DBを開き、CRUD・検索・同期を提供する。macOSは同一Team / Keychain access groupで署名する | ADR-011、Issue #55の共有profile coordination、production OS secret storeに依存。Issue #63でfail closed化し、実接続work item完了まで維持する |
| MCPサーバー実接続 | CLIと同じ共通client/profile層からタスクCRUD・検索・同期をstdio MCPとして公開する | FTS5 task-62、sync task-72、ADR-011、Issue #55に依存。Issue #63でprotocolをadvertiseしないfail-closed stubへ固定し、実接続work item完了まで維持する |
| サーバーのデバイス行重複排除 | 同一インストールからの再ログインで既存device rowを再利用する | 2026-07-10実機同期確認 |
| Android Keystore DeviceKeyStore | Androidの開発用 `FileDeviceKeyStore` を本番用Android Keystore実装へ置き換える | 技術仕様§4.3 / task-74 |
| Phase 1リリース前のlight固定 | ダークモード正式対応まで `themeMode` をlight固定する | 2026-07-06人間裁定 |
| iOSリリース準備 | 課金基盤のrelease gate合格後にrelease build、署名、ストア提出前コンプライアンスを確認する | M5-01。Billing foundation release gateと人間作業に依存 |
| macOS dogfooding配布 | macOS desktop buildを配布し、既知差分をリリースノートへ記録する | M5-02。人間判断を含む |
| クラッシュレポート方針 | F-53のオプトイン、PII除去、実送信有無を確定する | M5-03。法務・プライバシー判断を含む |

## Quick fixes

task文書へ昇格せず、軽量レーンで処理できる候補である。

- Closed見出しの `Closed 2 closed` のような冗長表記を整理する。出典: 2026-07-07親レビュー。

## Icebox

優先順位を付けない。四半期またはマイルストーン境界で残す価値を再評価する。

- 自然言語日付入力（`tomorrow` / `next Friday` / `明日` 等）。出典: task-52スコープ外。
- タスク行右側affordanceをchevron継続とするかFocus開始ボタンへ変えるか。出典: design direction / UI spec。
- リストへプロジェクト型 / エリア型の区別を導入するか。出典: 2026-07-07人間コメント。

## 人間作業・判断の詳細

- iOS実機でKeychain鍵保持、署名付きゼロプロンプト、通知、同期を確認する。出典: task-64 / 65 / 77。
- Android実機で同期動作を確認する。出典: task-74。
- AWS / ECR / Lambda / Neonへ本番デプロイし、WAF / API Gateway / CloudFront前段を確定する。出典: Phase 2計画書。
- 承認済み契約に従ってRevenueCat / App Store productを設定し、Test StoreとApple sandbox実機E2Eを完了する。実装状態はBilling foundation release gate work itemを正本とする。出典: Phase 2計画書 / `docs/billing_overview.md`。
- coral / amber系表示のコントラストを裁定する。出典: task-66。
- `~/.codex/config.toml` へ `~/.cargo` を追加するか判断する（任意）。

## 補充ルール

候補の供給源は次に限定する。

1. Phase計画書の未達マイルストーン
2. task完了報告の未解決事項
3. ADRの帰結・後続実装
4. ドッグフーディングで確認された具体的な問題
5. プロダクトオーナーの明示的な決定

出典のない候補を作らない。pilot中に新規候補が必要になった場合は、出典を本文へ記録した `status: backlog` の新形式work itemとして作成する。このlegacy backlogには追加しない。

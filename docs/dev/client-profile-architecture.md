# Taskveil client / frontend adapter architecture

この文書は、Flutter bridge、CLI、MCPからRust共通実装へ入る依存境界と命名規則を定める。local profile共有の設計判断はADR-011、同期protocolの正本は`docs/03_技術仕様書.md`とする。

## 用語

- `TaskveilClient`: 1つのlocal profileを開き、account/session、application service、transaction、同期を統括するfrontend-neutralなruntime facade。ユーザー表示profileではない。
- `LocalProfile`: SQLCipher DB、OS secret store上のDevice Key参照、永続account binding、local sync stateからなる端末内のデータ・security boundary。Rustの公開runtime struct名にはしない。
- `LocalProfileConfig`: `TaskveilClient::open`へ渡すlocal persistenceの起動設定。DB directoryとbootstrap値だけを持ち、credential、server URL、session state、永続identity bindingは持たない。
- `LocalProfileBinding`: storageへ永続化するaccount / tenant identity。`LocalProfileConfig`とは別の概念である。

## 目標構成

```mermaid
flowchart TB
  Flutter["Flutter UI"] --> Bridge["taskveil_app_bridge\nFRB functions + DTO mapping"]
  CLI["taskveil-cli\nclap I/O adapter"] --> Client
  MCP["taskveil-mcp-server\nstdio / MCP adapter"] --> Client
  Bridge --> Client["taskveil-client\nTaskveilClient + application services"]

  Client --> Domain["taskveil-domain\nentities + invariants"]
  Client --> Crypto["taskveil-crypto\nkey hierarchy + E2EE"]
  Client --> Protocol["taskveil-protocol\nwire DTO + versions"]
  Client --> Storage["taskveil-storage\nSQLCipher schema + repositories"]
  Client --> Sync["taskveil-sync\nHTTP transport + state machines"]
  Sync --> Protocol
  Client --> Secrets["OS secret store adapters"]
  Client --> DB[("SQLCipher profile DB")]
  Sync --> Server["E2EE sync server"]
```

依存方向はfrontend adapter → `taskveil-client` → 下位crateの一方向とする。Flutter、CLI、MCPはrepository、DB key、master key、tenant ID、`LocalMutationContext`、sync storeを受け取らない。`TaskveilClient`の高水準methodへtyped inputを渡し、frontend固有の入力・出力へ変換する。

## 所有責務

| 層 | 所有するもの | 所有しないもの |
|---|---|---|
| `taskveil_app_bridge` | FRB公開関数、文字列/typed input変換、Dart向けDTO変換、process内client handle | repository、SQLCipher open、鍵、account/sync state、同期順序、runtime生成 |
| `taskveil-cli` | clap、対話、表示、exit code | CRUD規則、repository、暗号、同期coordinator |
| `taskveil-mcp-server` | MCP schema、認可prompt、stdio transport、tool response | CRUD規則、repository、暗号、同期coordinator |
| `taskveil-client` | local profile open、account/session、application service、transaction境界、sync coordinator、SQLite sync adapter | Flutter/Dart/FRB、clap、MCP transport |
| `taskveil-domain` | entity、不変条件、純粋な状態遷移 | DB、network、frontend |
| `taskveil-storage` | schema、migration、repository、transaction primitive | frontend、network同期順序 |
| `taskveil-protocol` | sync/account/Organizationのserde DTO、wire enum、canonical wire value、version/constants | storage、HTTP、暗号演算、domain merge、client runtime |
| `taskveil-sync` | HTTP transport、E2EE record、merge、同期state machine/trait | Flutter、具体SQLite repository、profile UI |

## Fuzzy-scanの配置

- stable-key page、delta、high-water closureのwire contractは`taskveil-protocol`、mark/sweepを含むstate machine/traitは`taskveil-sync`。
- resync generation、preflight、lease、crash recovery、実行順序は`taskveil-client`。
- cursor/mark table/schema/transactionは`taskveil-storage`。
- SQLite trait adapterは`taskveil-client`。
- server current-state scanとGC horizonは`taskveil-server`。
- Flutter bridgeは`client.sync_now`と`client.sync_status`以外のFuzzy-scan実装を持たない。

このためFuzzy-scanを追加しても、FRB公開APIやFlutter側Rust adapterを変更せずに実装できることを設計レビュー条件とする。

## Foreground realtime通知の配置

ADR-019のWebSocketは同期transportではなく、foreground app lifecycleへ従う欠落可能なwake-up hintである。境界は次に固定する。

- `taskveil-client`は現在のaccount / session / tenant contextを使って短命realtime ticketを取得し、frontend-neutralな`RealtimeTicket`として返す。session token、tenant ID、device ID、HMAC keyをfrontendへ公開しない。
- `taskveil_app_bridge`はticket DTO変換とasync委譲だけを持つ。WebSocket、reconnect timer、sync scheduler、HTTP client、secretを保持しない。
- Flutterはforeground / background lifecycle、WebSocket接続、ticket refresh、reconnect backoff、notification frame decodeを所有する。frameからdomain / sync stateを解釈せず、固定`changed` hintを受けたら既存`sync_now`をrequestする。
- sync対象local mutation後の250ms debounce、single-flight dirty follow-up、接続中5分safety pull、切断中30秒fallback pollingもFlutter runtime orchestrationとする。CAS、merge、cursor、outbox ACK、continuityは引き続き`taskveil-client` / `taskveil-sync`だけが所有する。
- remote pull適用後は既存`SyncStatusNotifier`からlist / task / Home / Calendar / search / timer providerをinvalidateし、domain stateをWebSocket frameから組み立てず通常のrepository readでUIを更新する。
- CLI / MCPへWebSocketを強制しない。必要になった場合も`RealtimeTicket`を共通入口とし、各frontend lifecycleに適したadapterを別途実装する。

この配置はFlutterへ同期correctnessを移さない。WebSocketを完全に削除しても、明示sync、resume sync、fallback pollingと既存HTTPS state machineだけで最終収束することをレビュー条件とする。

## crate命名

`core/`はCargo workspace内の配置ディレクトリで、crateではない。次を正規形とする。

```text
directory:     core/<role>
Cargo package: taskveil-<role>
Rust import:   taskveil_<role>
```

`[package] name = "core"`、`[lib] name = "core"`、dependency alias `core = { ... }`、曖昧なumbrella `taskveil-core` crate、雑多なroot module `mod core`を追加しない。Rust標準の`::core`と名前を競合・混同させないためである。

`taskveil_app_bridge`はCargo package、lib target、FRB stem、pod名が一致する既存のビルド契約なので改名しない。

## レビューと機械的check

新しいfrontend機能は次の順で実装する。

1. `taskveil-client`へfrontend-neutralなinput/output/errorとapplication serviceを追加する。
2. domain/storage/syncをまたぐtransactionと回帰testをclient側で完成させる。
3. Flutter/CLI/MCP adapterへ薄い入出力変換を追加する。

`sh app/tool/check_client_boundaries.sh`はfrontend manifestの直接依存、bridge sourceの禁止import、bare `core` crate/aliasを検査する。Cargo compileだけでは検知できない境界の意図をCIで固定する。

task-92で`app/rust/src/support.rs` / `sync_store.rs`を削除し、application/local profile責務を当時の`ClientProfile`へ全面移設した。task-93でnetwork FRB関数もasyncへ統一し、bridge内blocking executorを削除した。task-94でruntime facadeを`TaskveilClient`、起動設定を`LocalProfileConfig`、内部transaction primitiveを`SqliteMutationService`へ改名し、旧名aliasを残さず役割を分離した。bridgeの通常依存はFRBと`taskveil-client`だけで、Taskveil workspace内依存はclientのみであり、legacy exceptionは存在しない。CIは下位crate参照0、runtime生成0、削除済みmoduleの再作成禁止、manifest allowlistを検査する。Fuzzy-scanはこの境界を変えずに`taskveil-sync` / `taskveil-storage` / `taskveil-client` / serverへ実装する。

Networkを伴うaccount/sync APIは`TaskveilClient`とFRBの両方でasyncとし、Futureを直接awaitする。Flutter/Dart、CLI、MCPの各runtime内で自然に実行し、adapterやclientがnested runtimeを生成しない。低水準の`SqliteMutationService`、`LocalMutationContext`、SQLite sync store、local crypto helperは通常public APIではなく、server統合testが`test-support` featureを明示した場合だけ利用できる。

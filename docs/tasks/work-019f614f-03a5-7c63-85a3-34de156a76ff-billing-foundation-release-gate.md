---
id: 019f614f-03a5-7c63-85a3-34de156a76ff
title: Billing foundation release gate
status: active
lane: critical
milestone: M5
---

# Billing foundation release gate

## 1. 背景とコンテキスト

Taskveilは一般リリース前であり、E2EE同期を含む主要な製品基盤は実装済みである。一方、課金実装は`server/src/routes/billing.rs`のTODOを含め未着手で、購入・復元、サーバー側検証、エンタイトルメント反映、失効時の同期制御がend-to-endで成立していない。プロダクトオーナーは2026-07-15に、課金基盤が完成するまで最初の一般リリースを行わないと決定した。

## 2. 事前に読むべきファイル

- `AGENTS.md`
- `docs/tasks/PLAYBOOK.md`
- `docs/01_企画書.md` §6、§8
- `docs/03_技術仕様書.md` §6.1、§10
- `docs/billing_overview.md`
- `docs/07_Phase1計画書.md` M5
- `docs/08_Phase2計画書.md`
- `server/src/routes/billing.rs`
- account、session、sync authorization、Flutter account UIの現行実装とテスト

## 3. ゴール

購入または復元された有効な利用権をサーバーが検証し、アカウントのエンタイトルメントへ冪等に反映し、クライアント表示と同期APIの認可へ一貫して適用できる課金基盤を完成させる。ローカル機能とローカルデータは課金状態に依存させない。

初回対象はiOSの個人向けProだけとする。RevenueCatをprovider adapterの初期実装とし、月額・年額商品を単一の`pro` entitlementへ束ねる。最終価格は2026-07-16に人間承認済みとし、具体額をコードやpublic文書へ固定せずStore metadataから表示する。

## 4. スコープ

### やること

- iOSの購入・復元フローと、購入結果をサーバー検証へ渡すクライアント境界を実装する。
- RevenueCat Test StoreとApple sandboxを段階的に使い、provider SDKの結果ではなくserverが再取得したsnapshotを認可へ反映する。
- provider固有payloadを正規化するbilling adapter、署名・真正性検証、event重複排除、監査可能な処理結果をサーバーへ実装する。
- subscription / entitlementの永続状態と状態遷移を実装する。
- 同期APIがリクエストごとにserver-side entitlementを検証する。
- active、trial、grace、expired、revoked、復元、返金、重複event、順不同eventを自動テストする。
- Flutterで購入、復元、現在状態、失敗、失効を静かで文脈的なUIとして表示する。
- sandbox環境で購入・復元・更新・失効をend-to-end確認し、リリースゲートの証拠を記録する。
- RevenueCatのcustom App User IDをTaskveil userごとにserverで生成し、匿名ID、email、直接の`user_id`をprovider identityに使わない。

### やらないこと

- 具体価格、売上予測、launch offer、provider手数料比較をpublic repoへ記録しない。
- Organization課金、seat課金、Web課金、Android課金を最初のiOSリリース必須範囲へ含めない。
- ローカルCRUD、ローカル検索、Calendar、Timer、テンプレート等を有料化しない。
- クライアントが提示したplanや未検証receiptを認可の正本にしない。
- RevenueCat Paywalls、匿名購入、launch offer、独自7日graceを実装しない。
- 購入payload、メールアドレス、token、鍵、復号済みコンテンツをログや完了報告へ含めない。

## 5. 実装手順

1. 本work itemとpublic/private仕様へ、下記の承認済み契約を固定し、実装前に独立レビューする。
2. `purchases_flutter`、RevenueCat API、webhook署名鍵、secret管理の供給網・権限境界をレビューし、導入前承認を記録する。
3. server schema、event ingestion、検証adapter、冪等な集約、entitlement query、sync authorizationを実装する。
4. `taskveil-client`へfrontend-neutralなentitlement APIを置き、FRBをtyped DTO変換と薄い委譲に限定する。
5. Flutterの購入・復元・状態表示をARB、accessibility、失敗復帰込みで実装する。
6. unit / integration / widget / sandbox E2Eを実行し、失効後もローカル編集可能であることを確認する。
7. 統合HEADを独立検証し、課金・実機・法務・運用のリリースチェックリストをすべて閉じてから一般リリース準備へ進む。

### 5.1 承認済みproduct契約

| 項目 | 契約 |
|---|---|
| provider | RevenueCat |
| entitlement / offering | `pro` / `default` |
| monthly product | `com.taskveil.app.pro.monthly` |
| yearly product | `com.taskveil.app.pro.yearly` |
| trial | 初回商品には無料trialを設定しない。RevenueCatの`trialing`状態は将来の導入やprovider snapshotの安全な処理のため状態機械で引き続き扱う |
| grace | App Store標準の16日Billing Grace Period。RevenueCatの`in_grace_period` / `gives_access`を使用 |
| restore | RevenueCat `Keep with original App User ID`。別Taskveil accountへの移管を拒否 |
| launch offer | 初回リリースではなし |
| price | 2026-07-16に人間承認済み。具体額はprivate課金設計を正本とし、コードへ固定せずStore metadataから表示 |

### 5.2 server data / API契約

`users.plan`を廃止し、次のcontrol-plane tableを作る。すべてUUIDはPostgres `gen_random_uuid()`のUUIDv4、時刻は`timestamptz`、provider enum相当のCHECK値は初回`revenuecat`だけとする。billing tableはE2EE content、email、鍵材料を持たない。

| table | column / constraint |
|---|---|
| `billing_customers` | `user_id uuid primary key references users(id) on delete cascade`、`provider text not null check (provider='revenuecat')`、`provider_app_user_id uuid not null unique`、`sandbox_refresh_token uuid`、`production_refresh_token uuid`、`created_at timestamptz not null default now()`。account登録transaction内で作成し、既存userはmigrationでbackfillする。provider App User IDは不変で、lazy再生成しない。refresh tokenは環境ごとにprovider取得開始時に更新し、最新tokenを持つsnapshotだけをapplyできる |
| `billing_events` | `id uuid primary key default gen_random_uuid()`、`provider text not null check (provider='revenuecat')`、`project_id text not null`、`provider_event_id text not null`、`app_id text not null`、`environment text not null check (environment in ('sandbox','production'))`、`provider_app_user_id uuid not null`、`event_type text`、`store text`、`store_product_identifier text`、`store_transaction_identifier text`、`store_original_transaction_identifier text`、`price numeric(12,4)`、`currency char(3)`、`country_code char(2)`、`payload_sha256 bytea not null check (octet_length(payload_sha256)=32)`、`processing_status text not null default 'processing' check (processing_status in ('processing','processed','failed'))`、`processing_started_at timestamptz not null default now()`、`processing_error_code text`、`received_at timestamptz not null default now()`、`processed_at timestamptz`。`unique(provider, project_id, provider_event_id)`。transaction fieldは監査用nullable fieldで、raw payloadとsubscriber attributesは保存しない |
| `billing_subscriptions` | `id uuid primary key default gen_random_uuid()`、`user_id uuid not null references users(id) on delete cascade`、`provider text not null check (provider='revenuecat')`、`environment text not null check (environment in ('sandbox','production'))`、`provider_subscription_id text not null`、`store_transaction_identifier text`、`store_original_transaction_identifier text`、`store_product_identifier text not null`、`provider_product_id text not null`、`status text not null check (status in ('trial','active','grace','expired','revoked'))`、`gives_access boolean not null default false`、`current_period_ends_at timestamptz`、`access_expires_at timestamptz`、`will_renew boolean`、`revocation_reason text`、`provider_observed_at timestamptz not null`、`last_seen_at timestamptz not null`、`updated_at timestamptz not null default now()`。`unique(provider, environment, provider_subscription_id)`。snapshot所有権はRevenueCat subscription IDで固定し、別user snapshotで同じIDをupsertした場合はreplayとして全更新を拒否。current store transaction IDはRevenueCat v2 `store_subscription_identifier`から監査用に保持し、webhookの同一transactionと正確に相関できた場合だけoriginal transaction IDも保持する。いずれも認可・一意性keyに使わない |
| `account_entitlements` | `user_id uuid not null references users(id) on delete cascade`、`environment text not null check (environment in ('sandbox','production'))`、`lookup_key text not null check (lookup_key='pro')`、`status text not null default 'free' check (status in ('free','trial','active','grace','expired','revoked'))`、`gives_access boolean not null default false`、`source_subscription_id uuid references billing_subscriptions(id) on delete set null`、`store_product_identifier text`、`expires_at timestamptz`、`grace_expires_at timestamptz`、`will_renew boolean`、`provider_observed_at timestamptz`、`updated_at timestamptz not null default now()`、`primary key(user_id, environment, lookup_key)` |

RevenueCat project / app / secret key / public SDK keyは`sandbox`と`production`で完全に分離し、server startupで2環境のproject IDまたはapp IDが同一ならconfiguration errorとして停止する。Test StoreとApple sandbox buildはsandbox projectのpublic SDK keyだけ、Store提出buildはproduction projectのpublic SDK keyだけを使う。同じcustom App User ID文字列を両projectで使ってもcustomer resourceとactive entitlementはproject境界を越えず、serverはrequestのbilling environmentに対応するproject以外へ問い合わせない。これによりenvironment fieldを持たない`active_entitlements`を異なる環境間で共有しない。

Customer refreshは対象environment専用projectのsecret keyで次を呼び、list endpointは`next_page`がなくなるまで取得する。

1. `GET /v2/projects/{project_id}/customers/{provider_app_user_id}/subscriptions?environment={sandbox|production}&limit=100`（`customer_information:subscriptions:read`）
2. `GET /v2/projects/{project_id}/customers/{provider_app_user_id}/active_entitlements?limit=100`（`customer_information:customers:read`）
3. 各subscriptionの`product_id`に対する`GET /v2/projects/{project_id}/products/{product_id}`（`project_configuration:products:read`、process内で短時間cache可）

subscription内のentitlement `lookup_key='pro'`とactive entitlementの`entitlement_id`を照合し、その`expires_at`を実際のaccess deadlineとする。`in_grace_period`ではこの値を`access_expires_at` / aggregate `grace_expires_at`へ、`trialing / active`では`access_expires_at` / aggregate `expires_at`へ保存する。active entitlementがない、またはsubscriptionの`gives_access=false`ならaccessを与えない。`provider_observed_at`はprovider fieldではなく、完全snapshot取得が終わったserver refresh時刻である。

`pro`はRevenueCat entitlementの`lookup_key`、`com.taskveil.app.pro.monthly` / `yearly`はProduct responseの`store_identifier`として照合し、RevenueCat内部の`entl...` / `prod...` IDと混同しない。unknown status / product / entitlement、temporary promotional grantはfail closedとする。

RevenueCat statusは`trialing -> trial`、`active -> active`、`in_grace_period -> grace`、`in_billing_retry / paused / incomplete / expired -> expired`へ正規化し、unknownはfail closedとする。cancel済みでも`gives_access=true`かつ`current_period_ends_at`が未来なら`active`、refundまたはownership transfer awayは即`revoked`とする。

refundはwebhookの`transaction_id`とRevenueCat v2 snapshotの`store_subscription_identifier`が一致するsubscriptionだけを`revoked`にする。商品IDだけで複数subscriptionを失効させず、遅延した返金eventが後発の同一商品再購入を巻き込まない。refund revocation後、同じprovider subscription IDに新しいstore transaction IDと有効なaccessが現れた場合は有償renewalとして復帰を許可する。ownership transfer awayはrevocationを維持する。

完全pagination済みsnapshotには`refresh_started_at`を付け、同じuser / provider / environmentでsnapshotに現れなかった既存subscriptionを`expired`、`gives_access=false`へ更新する。部分取得・provider error時はこのreconciliationを行わない。同一userに複数subscriptionがある場合、対象environment・対象2商品だけを候補とし、`gives_access=true`かつdeadlineが未来の候補から最も遅いaccess deadlineを第一順位、同deadlineなら`grace > active > trial`を第二順位としてaggregateへ採用する。provider snapshot取得後、`billing_customers`行を`FOR UPDATE`してsubscription upsert、完全snapshot reconciliation、entitlement再計算、event完了を1 transactionで行う。

同一customer / environmentの同時refreshは、provider取得前に環境別refresh tokenを更新する。apply時に`billing_customers` の現行tokenとsnapshotはtokenを照合し、後から開始したrefreshによってsupersedeされたsnapshotは503で破棄する。これにより、遅延したactive snapshotが後発のrefund / expiryを巻き戻さない。

request-time同期許可predicateは次のすべてを満たす場合だけtrueとし、保存済みboolだけを信頼しない。

1. request serverのbilling environmentとaggregate environmentが一致する。
2. `gives_access = true`かつstatusが`trial / active / grace`である。
3. `trial / active`は`expires_at > server_now`、`grace`は`grace_expires_at > server_now`である。
4. source subscriptionが同じuserに所有され、`revocation_reason`がない。

### 5.3 HTTP / client contract

- `GET /v2/tenants/{tenant_id}/billing`はsession認証、personal tenant membership、`owner_user_id == auth.user_id`を確認し、billing customerを返す。登録transaction / migrationで必ず作るためlazy createしない。
- `POST /v2/tenants/{tenant_id}/billing/refresh`はbodyなしで同じ認証を行い、serverがprovider snapshotを再取得する。client receipt、product、App User IDを入力として受けない。
- 両endpointの200 responseは次のJSONで固定する。nullable fieldは省略せず`null`を返し、時刻はUTC epoch millisecondsとする。

```json
{
  "provider": "revenuecat",
  "provider_app_user_id": "00000000-0000-4000-8000-000000000000",
  "entitlement": {
    "lookup_key": "pro",
    "status": "free",
    "sync_allowed": false,
    "store_product_identifier": null,
    "expires_at": null,
    "grace_expires_at": null,
    "will_renew": null,
    "environment": "sandbox",
    "updated_at": null
  }
}
```

- billing GET / refreshは401 invalid session、403 non-personal/non-owner、503 provider unavailableを返す。provider未取得または対象subscriptionなしは200 `free`である。
- personal tenantの全sync endpointとrealtime ticket発行はrequest-time predicateがfalseならHTTP 402 `{"error":"entitlement_required"}`を返す。
- 402は`SyncEngineError::EntitlementRequired` / `AccountClientError::EntitlementRequired` → `ClientError::EntitlementRequired` → FRBのtyped `BillingRequired` outcomeへ保持し、Flutter providerが課金導線を表示する。generic `sync failed`文字列へ潰さない。local CRUDはこのerror pathを通らない。

### 5.4 webhook / secret / failure契約

- serverは`TASKVEIL_BILLING_ENVIRONMENT`で`sandbox / production`の一方だけを選び、選択環境のRevenueCat project ID、read-only secret key、allowed app ID、webhook Authorization値、webhook signing secretだけを読む。stagingはsandboxだけ、productionはproductionだけを保持し、非選択環境のsecretを起動要件にしない。clientもbuild environment別のpublic SDK keyをsecret store / CI設定から受け取る。実値はcommitせず、同じproject / appをsandboxとproductionで兼用しない。[RevenueCat webhook integration](https://www.revenuecat.com/docs/integrations/webhooks#webhook-signature-verification-hmac)でHMAC signingを有効化し、RevenueCatが直接付与する`X-RevenueCat-Webhook-Signature`をserverで検証する。追加のsigning ingressは置かない。
- productionにbilling bypassを置かない。testはfake providerとDB fixture、手動開発はRevenueCat Test Storeを使う。
- `POST /v1/billing/webhooks/revenuecat`はAuthorization値をconstant-time比較し、`X-RevenueCat-Webhook-Signature: t=<unix>,v1=<hex>`をparseして`<t>.<raw body bytes>`のHMAC-SHA256をconstant-time比較する。server clockとの差が300秒を超えるdeliveryを拒否する。
- missing / malformed Authorization・signature・stale timestampは401、JSON不正・必須field不正は400、app / environment不一致は403、unknown customerは404、provider snapshot失敗は503とする。どの場合もaggregateを変更しない。project IDはpayloadに存在しないためserver設定上のintegration identityとして扱い、payload比較はしない。webhookの`SANDBOX / PRODUCTION`はDBの`sandbox / production`へ明示的に正規化する。customer解決は`app_user_id`、`original_app_user_id`、`aliases`の順に既知UUIDを探索し、複数userへ解決した場合は拒否する。
- event rowを`processing`でclaimし、処理済みduplicateは200、`processing`が2分未満の同時duplicateは503、`failed`または2分以上staleなclaimは再取得して処理する。provider snapshot成功後だけaggregate transactionと同時に`processed`へする。
- provider refresh失敗時は既存aggregateを勝手に延長・失効させず、保存済み期限とserver clockで認可する。
- RevenueCat Authorization値はserver設定とRevenueCat dashboardを同一maintenance windowで更新する。HMAC secretはRevenueCat dashboardでrotateすると旧secretが即時無効になるため、新secretを取得後ただちにserver secretを更新・deployし、短い不一致期間のdeliveryはRevenueCat retryで回収する。通常rotationでdual-secret期間を仮定せず、どちらもtest webhook成功まで監視する。

## 6. 受け入れ基準

- iOS sandboxで購入と購入復元が成功し、同一アカウントのserver entitlementへ反映される。
- receipt / transaction / webhookはserver側で検証され、改ざん、不正署名、別accountへのreplayが拒否される。
- event IDとtransaction IDの重複処理が冪等で、順不同eventからも決定済み状態規約へ収束する。
- active / trial / graceでは許可され、expired / revokedでは同期APIがserver側で拒否する。
- sandbox entitlementがproduction requestを許可せず、client cacheやRevenueCat SDK表示を改ざんしてもHTTP 402を迂回できない。
- 課金失効後も既存local DBの閲覧・編集・local-only機能が継続し、再有効化後に通常同期へ復帰する。
- クライアントの表示状態を改ざんしてもserver-side authorizationを迂回できない。
- 購入、復元、失敗、保留、失効のUIがen/ja、accessibility、再起動復元を含めて検証される。
- repositoryの共通品質ゲート、課金固有テスト、独立検証、iOS sandbox E2Eが合格する。
- 課金基盤が未完了の間、store提出、release tag、公開告知を行わないことが運用文書と一致している。

## 7. 制約・注意事項

- `lane: critical` とし、新規SDK / dependency、課金状態規約、保存schema、secret運用は実装前に人間承認を得る。
- 本work itemのproduct、価格、初回trialなし、grace、restore、launch offer、provider判断は2026-07-16に承認済みである。無料trialを将来追加する場合は別途判断し、初回商品設定へ暗黙に追加しない。外部product設定とApple sandbox E2Eは引き続き停止条件とする。
- public repoには実装・検証に必要な公開可能情報だけを置き、価格、収益、契約、provider比較の詳細を置かない。
- エンタイトルメントの正本はserver-side aggregateとし、外部providerは更新sourceとして抽象化する。
- pre-release方針に従い、未実装の旧経路や仮planを残すcompatibility layerを追加しない。
- App Store、課金provider、署名鍵、webhook secretの実値をcommitしない。

## 8. 完了報告に含めるべき内容

- 実装した購入、検証、event集約、entitlement、同期認可、Flutter UIの概要
- schema / API / dependency / secret運用に関する承認済み判断
- unit / integration / widget / sandbox E2Eと独立検証の結果
- activeから失効・復元までの観測証拠と、local-only機能を維持した証拠
- 一般リリースへ進めるかのゲート判定と、残る人間作業
- RevenueCat Test StoreとApple sandboxを区別した次のscenario結果: 月額、年額、同一account復元、別account拒否、renewal、cancel、billing retry、grace、recovery、expiry、refund、再購入、失効中local edit、再有効化後sync

## 9. 実装・検証記録（2026-07-16）

### 9.1 実装結果

- public実装commit: `781cdad`（`feat: implement billing foundation`）
- `users.plan`を削除し、`billing_customers`、`billing_events`、`billing_subscriptions`、`account_entitlements`をmigrationで追加した。
- RevenueCat API v2 adapter、完全pagination、environment / app / product検証、unknown statusのfail closed、同時refresh token、subscription所有権固定、refund / transfer / 再購入規約を実装した。
- webhookのAuthorization、raw-body HMAC、5分tolerance、event claim / retry / duplicate処理を実装した。eventから直接遷移せず、provider snapshotで集約する。
- billing GET / refreshと、personal tenantの全sync routeおよびrealtime ticketにrequest-time entitlement判定を実装した。未許可時はHTTP 402 `entitlement_required`をtyped errorのままFlutterまで保持する。
- Rust clientへbilling bootstrap / refresh / SQLCipher表示cacheを追加した。Flutterはserver発行App User IDだけでRevenueCat SDKを構成し、購入・復元後はserver refreshが成功するまで同期許可へ切り替えない。
- Account画面へen/jaのPro表示、Store localized metadata、購入・復元・管理導線、全状態表示、large textとscreen reader semanticsを追加した。`purchases_flutter 10.4.1`を固定し、iOS SwiftPM解決fileを記録した。
- public技術仕様とprivate課金設計を同期し、独自7日graceと認可用14日client cacheを廃止した。価格、secret、provider運用の非公開値はpublic repoへ記録していない。

### 9.2 検証証拠

- `cargo fmt --all -- --check`: PASS
- `cargo clippy --workspace -- -D warnings`: PASS
- `cargo test --workspace -q`: PASS。billing integration 9件、全sync / realtime route、client cache再起動・破損拒否を含む。
- `cd app/rust && env CARGO_TARGET_DIR=target cargo build --release`: PASS
- `cd app && flutter analyze`: PASS
- `cd app && flutter test`: PASS（253件、Visual QA harness 1件は通常実行時skip）
- `sh app/tool/check_hardcoded_strings.sh`: PASS
- `sh app/tool/check_client_boundaries.sh`: PASS
- `sh app/tool/test_client_boundaries.sh`: PASS
- `cd app && env TASKVEIL_VISUAL_QA=1 flutter test test/visual_qa/visual_qa_screenshots_test.dart --name billing_`: PASS（Free / en、grace / ja / text scale 2.0を目視確認）
- `cd app && flutter build ios --simulator --no-codesign`: PASS（`Runner.app`生成）
- local CRUD / search / Calendar / Timerはbilling predicateを通らない構造を維持し、全client / Flutter suiteがPASSした。失効状態を使う実機local editと再有効化後syncは下記外部E2Eで確認する。

### 9.3 独立検証

別担当が統合差分をread-onlyで検証し、古いsnapshot競合、unknown status、transfer、refund相関、全route 402、SQLCipher cache、Flutter fallback / localized metadata / accessibilityを再実行した。最終判定はPASSで、未解消のP1 / P2 / P3指摘はない。

### 9.4 未完の外部ゲートと判定

- sandbox / productionのRevenueCat project、app、API key、offering `default`、entitlement `pro`、`Keep with original App User ID`の設定
- RevenueCat Test Storeの月額・年額、同一account復元、別account拒否、cancel、refund、再購入scenario
- RevenueCat webhook integrationのAuthorization設定、HMAC signing有効化、環境別signing secret投入、direct test webhook
- App Store商品をintroductory offerなしで作成し、16日Billing Grace Periodを設定
- Apple sandbox実機でrenewal、billing retry、grace、recovery、expiry、refund、失効中local edit、再有効化後syncを含む全scenarioの証跡

コード差分とrepository内検証は受け入れ可能である。ただし上記外部ゲートが未完のためfront matterは`active`を維持し、一般リリースゲートは閉じない。store提出、release tag、公開告知へ進んではならない。

### 9.5 初回trial方針の更新（2026-07-16）

- プロダクトオーナー判断により、初回iOS Proは無料trialなしへ変更した。App Store商品にはintroductory offerを設定せず、購入画面から14日trialの説明と読み上げ文言を削除した。
- RevenueCatの`trialing` snapshotをfail closedにせず正規化できるよう、server schema、状態機械、認可、既存testの`trial`対応は将来互換として維持した。Store設定に存在しないtrialをclientが表示または生成する経路は置いていない。
- `flutter gen-l10n`、Rust bridge release build、`flutter analyze`、`flutter test`（253件PASS、Visual QA harness 1件skip）、hardcoded string / client boundary checkを再実行し、すべてPASSした。billing専用Visual QA 2件も生成し、英語Free購入画面と日本語grace・text scale 2.0にoverflowがないことを目視確認した。
- 別担当のread-onlyレビューはPASSで、P1 / P2 / P3指摘なし。初回trialなしのpublic/private整合、14日trial訴求の除去、serverの`trialing -> trial`互換維持、public/private境界を確認した。
- 外部設定・sandbox E2Eは未完のため、work itemは引き続き`active`とする。

### 9.6 初回価格の承認（2026-07-16）

- プロダクトオーナーが初回iOS Proの月額・年額価格を承認した。具体額、割引率、収益前提はprivate課金設計を正本とし、public repoへ転記しない。
- clientは引き続きStoreのlocalized metadataだけを表示し、価格をDart / Rustへ埋め込まない。価格改定でもproduct ID、RevenueCat offering / entitlement、server認可契約を変更しない。
- 独立レビューでlegacy `BACKLOG.md`に残った承認待ち表現をP2として検出し、未完の外部設定・sandbox E2Eだけへ修正した。再レビューはPASSで、未解消のP1 / P2 / P3指摘はない。
- 価格承認の停止条件は解消した。RevenueCat / App Store product設定とsandbox実機E2Eが未完のため、work itemは`active`を維持する。

### 9.7 Android client同等化（2026-07-25）

- プロダクトオーナーのAndroid / iOS実装同等化承認に基づき、`work-019f9947-43fe-7fd0-804d-13f7b5beb01f-android-ios-parity`でRevenueCat Android public SDK key、Google Play購入・復元・subscription管理URL、store障害時も最新server entitlementを保持するclient境界を追加した。
- 認可の正本、custom App User ID、environment分離、購入後のserver refresh、local-only継続契約はiOSと同一であり、Google Playのclient receiptやSDK表示を認可へ使用しない。
- 本work itemの初回iOS実装記録は履歴として維持するが、一般リリースgateはApp StoreとGoogle Playの各sandbox E2Eを要求する。RevenueCat Android app、Google Play商品、upload signing、sandbox購入・復元・更新・失効・返金・再有効化は外部作業であり、Emulator testや固定test entitlementで成功扱いにしない。
- App Store / Google Play双方の外部gateが未完のため、work itemは引き続き`active`とし、store提出、release tag、公開告知へ進まない。

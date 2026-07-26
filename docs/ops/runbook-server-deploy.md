# staging server deploy runbook

Taskveil server / realtime Workerをstagingへ反映する順序と停止条件を定義する。実AWS / Cloudflare / Neonのaccount ID、domain、resource ID、credentialはpublic repoやCI logに記録しない。

## 1. 正本と前提

- 構成の正本は[`docs/03_技術仕様書.md`](../03_技術仕様書.md) §1.5、[`docs/05_設計判断記録.md`](../05_設計判断記録.md) ADR-008 / ADR-022、`infra/`と`.github/workflows/deploy-staging.yml`である。
- stagingは`eu-central-1`のx86_64 Lambda、API Gateway HTTP API、Neon、Cloudflare Worker / Durable Objectsを使う。
- deployはGitHub staging Environmentの承認後だけOIDC roleを取得する。通常CIにcloud credentialは与えない。
- `server/Dockerfile`はportable image、`server/Dockerfile.lambda`はLambda Web AdapterとAWS Parameters and Secrets extensionを含むLambda用imageである。
- 初回OIDC / remote state / ECR bootstrap、secret投入、infra applyはcredentialを持つ人間の承認作業とする。bootstrap ECRへ初期imageをpushしてdigestを確定してから、Lambdaを含むstaging rootを初回applyする。
- 初回infra applyより前に、trafficを持たない`taskveil-realtime-staging` Worker service / versionとsecret bindingsを用意する。OpenTofuはそのserviceへCustom Domainを接続する。

## 2. secret境界

Secrets Managerは値をOpenTofu stateに入れず、containerだけを作る。

| secret | reader | JSONの用途 |
|---|---|---|
| runtime | Lambda、deploy role | pooled non-owner `DATABASE_URL`、RevenueCat sandbox、realtime server設定、認証limit HMAC key |
| migration | deploy roleのみ | owner `DATABASE_MIGRATION_URL` |
| deployment-provider | deploy roleのみ | Cloudflare deploy credentialなどprovider実行値 |

Lambda環境変数には`TASKVEIL_RUNTIME_SECRET_ID`、`TASKVEIL_BILLING_ENVIRONMENT=sandbox`、`TASKVEIL_TRUST_SOURCE_IP_HEADER=true`、正の整数の`TASKVEIL_AUTH_LIMIT_HMAC_KEY_GENERATION`、非秘密のlog / extension設定だけを置く。`DATABASE_URL`、`DATABASE_MIGRATION_URL`、RevenueCat / realtime key、`TASKVEIL_AUTH_LIMIT_HMAC_KEY`を直接置かない。認証limit HMAC keyはbase64化した独立32byte乱数としてruntime secretへ置き、raw値とHMAC出力をlogへ出さない。startup logで確認するのはgenerationだけであり、key materialやそのhashではない。

初回secret投入では、runtime JSONのrealtime current / previous ticket・publish keyと対応するkey IDをCloudflare staging Workerの8 secret bindingにもout-of-bandで投入する。値をprivate Git、GitHub variable、workflow outputへ置かず、server側とWorker側の組が一致することだけを確認する。

認証limit HMAC keyをrotationするときは通常deployと分離したmaintenance windowで次を順守する。

1. trusted ingressでregistration / login startを停止する。
2. OPAQUE stateの最大TTLを待ち、bounded cleanupを反復してglobal active capacityが0であることを確認する。
3. runtime secretのkeyを更新し、非秘密のgenerationを増加させる。
4. 新secretを読むLambda versionを発行し、weighted routingを設定せずaliasを新versionへ全量切替する。
5. generationのstartup eventとLambda metricsで、旧versionのinvocation / concurrencyが0へ収束したことを確認する。key materialとhashをlogへ出さない。
6. 以上が成立してから認証startを再開する。旧versionが残る場合は再開せず、alias / provisioned concurrency / event sourceを点検する。

GitHub `staging` Environmentには次の非秘密variableを設定する。値の正本はprivate deployment inventoryとし、GitHub secretへcloud長期credentialを置かない。

- 共通: `AWS_ACCOUNT_ID`、`BASE_DOMAIN`、`CLOUDFLARE_ZONE_ID`、`NEON_PROJECT_ID`
- bootstrap / infra: `STATE_BUCKET`、`INFRA_APPLY_ROLE_ARN`、`OIDC_PROVIDER_ARN`、`LAMBDA_BOOTSTRAP_IMAGE_URI`、`BUDGET_NOTIFICATION_EMAIL`
- deploy: `DEPLOY_ROLE_ARN`、`ECR_REPOSITORY`、`LAMBDA_FUNCTION`、`MIGRATION_SECRET_ARN`、`DEPLOYMENT_PROVIDER_SECRET_ARN`、`PARAMETERS_EXTENSION_LAYER_ARN`、`AUTH_LIMIT_HMAC_KEY_GENERATION`
- 自動化gate: `STAGING_AUTO_DEPLOY_ENABLED`（初期値は`false`）

`PARAMETERS_EXTENSION_LAYER_ARN`は`eu-central-1`のAWS公式x86_64 layerをversionまで固定したARNとする。workflow inputのcommitはfull SHAかつrepositoryの`main`履歴に含まれるものだけを許可する。

## 3. 事前検証

1. 対象をfull 40-character commit SHAで固定し、そのSHAのCIが成功していることを確認する。
2. `infra-check`、Rust / Flutter / security / client-boundary gateを通す。
3. Worker testと`wrangler deploy --dry-run`を行う。
4. deploy concurrencyの先行runがないことを確認する。migration中のrunはcancelしない。

## 4. deploy順序

workflowは次の順を変えない。

1. pinned AWS Parameters and Secrets extension layerを`tool/prepare_lambda_extension.sh`で`.lambda/extensions/`へ展開する。
2. `server/Dockerfile.lambda`を`linux/amd64`でbuildし、commit SHA tagをECRへpushしてdigestを固定する。
3. ECR enhanced/basic scanのCritical / Highが0であることを確認する。
4. Worker versionをuploadし、新version IDを記録する。Custom DomainはOpenTofu管理とし、version uploadへroute optionを渡さない。
5. OPAQUE capacity schemaを含むmigrationではtrusted ingressでregistration / login startを停止する。
6. migration secretを一時取得し、build済みimageの`taskveil-migrate`を実行する。値はmaskし、file、output、artifactに保存しない。
7. migration成功後だけLambda functionのimage digestを更新し、versionを発行してstaging aliasをweighted routingなしで新versionへ全量切替する。
8. Workerを新versionへdeployする。
9. smoke testを実行し、OPAQUE start / finishとcapacity invariantを確認する。
10. smoke成功後だけregistration / login startを再開する。

migrationが失敗した場合はLambda aliasとWorker deploymentを動かさず、停止した認証startも再開しない。migration適用後の旧Lambdaへのrollbackは認証start停止中だけ許可し、原則は前方修正migrationと新versionで復旧する。

## 5. smoke test

- `GET /health` → 200 `{"status":"ok"}`
- `GET /ready` → 200 `{"status":"ready"}`
- 保護APIの未認証request → 401
- Workerの不正ticket接続 → 拒否
- Workerの不正publish signature → 拒否

access logはrequest ID、route、status、latencyだけを確認する。body、Authorization、UUID、ticket、opaque identifier、暗号recordがCloudWatch / Cloudflare logにないことを確認する。

## 6. 失敗時とrollback

- migration失敗: deployを停止し、alias / Workerは現状維持する。
- alias切替後のsmoke失敗: capacity migration 005が未適用ならLambda aliasを直前version、Workerを直前deploymentへ戻し、smokeを再実行する。005適用済みなら認証start停止を維持し、旧aliasへのrollbackは非認証APIの緊急復旧に限定する。DB triggerにより旧versionでもglobal capは維持されるがper-identifier / application limitは成立しないため、旧versionでauth smokeや認証start再開を行わない。forward fix migrationと新Lambda versionを適用し、auth smoke成功後だけ再開する。
- DB: rollbackしない。必要な場合は前方修正migrationを追加する。

rollbackが失敗したら、[`runbook-incident.md`](./runbook-incident.md)へ移行する。

## 7. 自動deploy開放gate

次をすべて記録した後だけGitHub staging Environment variableの`STAGING_AUTO_DEPLOY_ENABLED=true`を設定する。

- 手動deployが3回連続成功した。
- 実端末2台の登録、ログイン、同期が成功した。
- realtime通知が同期を起動した。
- Cloudflare停止時もHTTPS fallbackで収束した。
- runtime DB role、RLS、EU jurisdiction、log allowlistを人間が確認した。

production apply / deploy workflowは作成しない。

# Taskveil infrastructure

OpenTofu 1.12.xでstaging / productionの境界を定義する。`bootstrap/state`がsecret containerだけを作成し、`modules/deployment`はそのmetadataを参照する。secret valueはvariable、plan、stateに取り込まない。

- `bootstrap/state`: accountごとのS3 remote state bucket、bootstrap ECR、GitHub OIDC、infra role、3つのsecret container。初回だけ人間のAWS credentialで実行し、その後secret valueをGit外から投入する。
- `environments/staging`: 実apply対象。GitHub staging Environment承認が必要。
- `environments/production`: 定義のみ。apply workflowはない。別AWS account、backend、Neon Projectを必須とする。

deployment moduleは`realtime.<environment>.<base-domain>`のWorker Custom Domainも管理する。初回applyより前に、対応する`taskveil-realtime-<environment>` Worker service / versionとsecret bindingsをtrafficなしで用意する。Custom Domainをversion uploadのCLI optionへ混在させない。

`${project}-${environment}/runtime` secret valueには、選択環境のDB / billing / realtime設定に加え、`TASKVEIL_RESYNC_TOKEN_KEY_CURRENT_ID`と32-byte standard-base64 `TASKVEIL_RESYNC_TOKEN_KEY_CURRENT`を必ず投入する。overlap rotation中だけ対応する`PREVIOUS_ID` / `PREVIOUS`も同時に保持し、旧鍵はtoken最大寿命24時間+clock margin 5分を経過してから削除する。値はOpenTofu variable、plan、stateへ渡さず、Secrets Managerへout-of-band投入する。

backend値と`*.tfvars`の実値はcommitせず、`*.example` を複製する。AWS account ID、Cloudflare zone ID、実domain、Neon Project ID、予算通知先はprivate運用台帳で管理する。

初回はbootstrapをapplyし、出力されたimmutable ECRへ同じcommitのLambda imageをpushしてdigestを得る。そのdigestを`lambda_image_uri`へ設定してからstaging rootをplan / applyする。ECRとLambdaを同じ初回applyで作成する循環は作らない。

```sh
tofu -chdir=infra/environments/staging init -backend=false
tofu -chdir=infra/environments/staging validate
```

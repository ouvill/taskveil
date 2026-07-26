#!/bin/sh
set -eu

fail() {
  echo "infra boundary check failed: $1" >&2
  exit 1
}

for root in infra/environments/staging infra/environments/production; do
  grep -q 'use_lockfile = true' "$root/backend.tf" || fail "$root must use native S3 locking"
done

if grep -R -n -E 'secret_string|aws_secretsmanager_secret_version' infra --include='*.tf'; then
  fail "secret values must not enter OpenTofu configuration or state"
fi

grep -q 'TASKVEIL_RUNTIME_SECRET_ID' infra/modules/deployment/lambda.tf || fail "Lambda must receive the runtime secret ID"
grep -Fq '"overwrite:header.x-taskveil-source-ip" = "$context.identity.sourceIp"' infra/modules/deployment/api.tf || fail "API Gateway must overwrite the trusted source IP header"
grep -q 'TASKVEIL_TRUST_SOURCE_IP_HEADER[[:space:]]*= "true"' infra/modules/deployment/lambda.tf || fail "Lambda must trust the source IP header only with the overwrite mapping"
grep -q 'TASKVEIL_AUTH_LIMIT_HMAC_KEY_GENERATION[[:space:]]*= tostring(var.auth_limit_hmac_key_generation)' infra/modules/deployment/lambda.tf || fail "Lambda must expose a nonsecret auth limit key generation"
if grep -q 'DATABASE_MIGRATION_URL' infra/modules/deployment/lambda.tf; then
  fail "Lambda runtime must not receive migration credentials"
fi

grep -q 'data.aws_secretsmanager_secret.runtime.arn' infra/modules/deployment/iam.tf || fail "Lambda runtime secret grant missing"
if sed -n '/data "aws_iam_policy_document" "lambda"/,/^}/p' infra/modules/deployment/iam.tf | grep -q 'migration'; then
  fail "Lambda policy must not reference the migration secret"
fi
grep -q 'eu-central-1:187925254637:layer:AWS-Parameters-and-Secrets-Lambda-Extension' tool/prepare_lambda_extension.sh || fail "extension download must bind the official Frankfurt publisher"
grep -q '187925254637:layer:AWS-Parameters-and-Secrets-Lambda-Extension' infra/modules/deployment/iam.tf || fail "deploy IAM must bind the official extension publisher"
grep -q 'COPY Cargo.toml Cargo.lock rust-toolchain.toml' server/Dockerfile || fail "portable image must use the workspace lockfile"
grep -q 'COPY Cargo.toml Cargo.lock rust-toolchain.toml' server/Dockerfile.lambda || fail "Lambda image must use the workspace lockfile"
grep -q '^FROM rust:slim@sha256:' server/Dockerfile || fail "portable builder image must be digest-pinned"
grep -q '^FROM rust:slim@sha256:' server/Dockerfile.lambda || fail "Lambda builder image must be digest-pinned"
grep -q '^FROM gcr.io/distroless/cc-debian12@sha256:' server/Dockerfile.lambda || fail "Lambda runtime image must be digest-pinned"
grep -q 'aws-lambda-adapter:0.9.1@sha256:' server/Dockerfile.lambda || fail "Lambda Web Adapter must be digest-pinned"
grep -q 'AWSParametersAndSecretsLambdaExtension' server/Dockerfile.lambda || fail "Lambda image must contain the secret extension"
grep -q 'resource "cloudflare_workers_custom_domain" "realtime"' infra/modules/deployment/worker.tf || fail "Worker custom domain must be managed by OpenTofu"
if grep -q 'versions upload.*--domain' .github/workflows/deploy-staging.yml; then
  fail "wrangler versions upload does not accept custom domain flags"
fi
grep -q 'install_actionlint.sh' .github/workflows/infra-check.yml || fail "infra checks must run pinned actionlint"

if grep -R -n -E 'tofu[[:space:]]+apply.*production|environments/production.*apply' .github/workflows --include='*.yml'; then
  fail "production apply workflow is prohibited"
fi

grep -q 'var.aws_account_id != var.staging_aws_account_id' infra/environments/production/variables.tf || fail "production must reject staging AWS account reuse"
grep -q 'var.neon_project_id != var.staging_neon_project_id' infra/environments/production/variables.tf || fail "production must reject staging Neon project reuse"

if grep -R -n -E 'uses:[[:space:]]+[^ @]+@(main|master|v[0-9]+)([[:space:]#]|$)' .github/workflows --include='*.yml'; then
  fail "GitHub Actions must be pinned by full commit SHA"
fi

for workflow in .github/workflows/infra-apply.yml .github/workflows/deploy-staging.yml; do
  grep -q 'git merge-base --is-ancestor.*origin/main' "$workflow" || fail "$workflow must reject commits outside main history"
done

grep -q 'token.actions.githubusercontent.com:sub' infra/modules/deployment/iam.tf || fail "OIDC subject boundary missing"
grep -q 'repo:${var.github_repository}:environment:${var.environment}' infra/modules/deployment/locals.tf || fail "OIDC subject must bind the GitHub Environment"

echo "infra boundaries: ok"

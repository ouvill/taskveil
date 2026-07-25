#!/bin/sh

set -eu

emulator_port=${EMULATOR_PORT:-5554}
device_id=${ANDROID_SERIAL:-emulator-$emulator_port}
repo_root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
parity_container="taskveil-android-parity-${GITHUB_RUN_ID:-local}-$$"
parity_run_id="${GITHUB_RUN_ID:-local}-$$"
parity_config=$(mktemp)
server_log=$(mktemp)
server_pid=

cleanup() {
    if [ -n "$server_pid" ]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    docker rm -f "$parity_container" >/dev/null 2>&1 || true
    rm -f "$parity_config" "$server_log"
}
trap cleanup EXIT INT TERM

cat >"$parity_config" <<EOF
{
  "TASKVEIL_ANDROID_PARITY_RUN_ID": "$parity_run_id",
  "TASKVEIL_ANDROID_PARITY_EMAIL": "android-parity-$parity_run_id@example.invalid",
  "TASKVEIL_ANDROID_PARITY_PASSWORD": "local-emulator-${parity_run_id}-credential",
  "TASKVEIL_ANDROID_PARITY_SERVER_URL": "http://127.0.0.1:8080"
}
EOF
chmod 600 "$parity_config"

adb -s "$device_id" wait-for-device
adb -s "$device_id" reverse tcp:8080 tcp:8080
device_abi=$(adb -s "$device_id" shell getprop ro.product.cpu.abi | tr -d '\r')
case "$device_abi" in
    arm64-v8a)
        flutter_target_platform=android-arm64
        ;;
    x86_64)
        flutter_target_platform=android-x64
        ;;
    *)
        echo "Unsupported Android Emulator ABI: $device_abi" >&2
        exit 1
        ;;
esac

cd "$repo_root/app"
flutter build apk --debug --target-platform "$flutter_target_platform"

cd android
./gradlew -Ptarget-platform="$flutter_target_platform" connectedDebugAndroidTest

cd ..
# connectedDebugAndroidTest removes the test application after it finishes.
# Reinstall the package so the permission state can be prepared before
# flutter drive replaces it with the profile build.
adb -s "$device_id" install -r \
    "$repo_root/app/build/app/outputs/flutter-apk/app-debug.apk"
adb -s "$device_id" shell pm revoke \
    com.taskveil.app android.permission.POST_NOTIFICATIONS 2>/dev/null || true
adb -s "$device_id" shell pm clear-permission-flags \
    com.taskveil.app android.permission.POST_NOTIFICATIONS \
    user-set user-fixed 2>/dev/null || true
adb -s "$device_id" shell pm grant \
    com.taskveil.app android.permission.POST_NOTIFICATIONS
flutter drive \
    --driver=test_driver/integration_test.dart \
    --target=integration_test/android_notification_test.dart \
    -d "$device_id" \
    --profile

flutter drive \
    --driver=test_driver/integration_test.dart \
    --target=integration_test/device_key_rotation_test.dart \
    -d "$device_id" \
    --profile \
    --keep-app-running
flutter drive \
    --driver=test_driver/integration_test.dart \
    --target=integration_test/android_ui_smoke_test.dart \
    -d "$device_id" \
    --profile \
    --keep-app-running \
    --dart-define-from-file="$parity_config"

cd "$repo_root"
TASKVEIL_DEV_POSTGRES_CONTAINER="$parity_container" \
TASKVEIL_DEV_POSTGRES_DB="taskveil_android_parity" \
TASKVEIL_BILLING_ENVIRONMENT=sandbox \
REVENUECAT_SANDBOX_PROJECT_ID=android-emulator-project \
REVENUECAT_SANDBOX_APP_ID=android-emulator-app \
REVENUECAT_SANDBOX_SECRET_KEY=android-emulator-secret \
REVENUECAT_SANDBOX_WEBHOOK_AUTHORIZATION=android-emulator-authorization \
REVENUECAT_SANDBOX_WEBHOOK_HMAC_SECRET=android-emulator-hmac \
TASKVEIL_AUTH_ISSUER=http://127.0.0.1:8080 \
./tool/dev_server.sh >"$server_log" 2>&1 &
server_pid=$!

server_ready=false
attempt=0
while [ "$attempt" -lt 90 ]; do
    if curl --fail --silent http://127.0.0.1:8080/health >/dev/null 2>&1; then
        server_ready=true
        break
    fi
    if ! kill -0 "$server_pid" 2>/dev/null; then
        sed -n '1,200p' "$server_log" >&2
        exit 1
    fi
    attempt=$((attempt + 1))
    sleep 1
done
if [ "$server_ready" != true ]; then
    sed -n '1,200p' "$server_log" >&2
    exit 1
fi

run_sync_phase() {
    phase=$1
    cd "$repo_root/app"
    flutter drive \
        --driver=test_driver/integration_test.dart \
        --target=integration_test/android_account_sync_test.dart \
        -d "$device_id" \
        --profile \
        --keep-app-running \
        --dart-define="TASKVEIL_ANDROID_PARITY_PHASE=$phase" \
        --dart-define-from-file="$parity_config"
}

run_sync_phase register

docker exec -i "$parity_container" \
    psql -v ON_ERROR_STOP=1 -U taskveil -d taskveil_android_parity <<'SQL'
WITH target_user AS (
    SELECT id
    FROM users
    ORDER BY created_at DESC
    LIMIT 1
),
subscription AS (
    INSERT INTO billing_subscriptions (
        user_id,
        provider,
        environment,
        provider_subscription_id,
        store_product_identifier,
        provider_product_id,
        status,
        gives_access,
        current_period_ends_at,
        access_expires_at,
        will_renew,
        provider_observed_at,
        last_seen_at
    )
    SELECT
        id,
        'revenuecat',
        'sandbox',
        'android-emulator-' || gen_random_uuid()::text,
        'com.taskveil.app.pro.monthly',
        'android-emulator-product',
        'active',
        TRUE,
        now() + interval '1 hour',
        now() + interval '1 hour',
        FALSE,
        now(),
        now()
    FROM target_user
    RETURNING id, user_id
)
INSERT INTO account_entitlements (
    user_id,
    environment,
    lookup_key,
    status,
    gives_access,
    source_subscription_id,
    store_product_identifier,
    expires_at,
    will_renew,
    provider_observed_at
)
SELECT
    user_id,
    'sandbox',
    'pro',
    'active',
    TRUE,
    id,
    'com.taskveil.app.pro.monthly',
    now() + interval '1 hour',
    FALSE,
    now()
FROM subscription;
SQL

run_sync_phase device_a_push
run_sync_phase device_b_roundtrip
run_sync_phase device_a_verify
flutter drive \
    --driver=test_driver/integration_test.dart \
    --target=integration_test/device_key_rotation_test.dart \
    -d "$device_id" \
    --profile

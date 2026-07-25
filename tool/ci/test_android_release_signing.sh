#!/bin/sh

set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT INT TERM

gradle_file="$repo_root/app/android/app/build.gradle.kts"
signing_report="$scratch/signing-report.txt"
failure_log="$scratch/store-signing-failure.txt"
missing_properties="$scratch/missing-key.properties"

if grep -F 'signingConfigs.getByName("debug")' "$gradle_file" >/dev/null; then
    echo "release must never fall back to the Android debug signing key" >&2
    exit 1
fi

(
    unset TASKVEIL_ANDROID_STORE_BUILD
    unset TASKVEIL_ANDROID_KEY_PROPERTIES
    unset TASKVEIL_ANDROID_KEYSTORE_PATH
    unset TASKVEIL_ANDROID_KEYSTORE_PASSWORD
    unset TASKVEIL_ANDROID_KEY_ALIAS
    unset TASKVEIL_ANDROID_KEY_PASSWORD
    cd "$repo_root/app/android"
    ./gradlew -PtaskveilStoreBuild=false signingReport
) >"$signing_report"

if ! awk '
    /^Variant: release$/ { in_release = 1; next }
    in_release && /^Config: null$/ { found = 1; exit }
    in_release && /^----------$/ { exit }
    END { exit(found ? 0 : 1) }
' "$signing_report"; then
    echo "ordinary release validation build must remain unsigned" >&2
    exit 1
fi

if (
    unset TASKVEIL_ANDROID_STORE_BUILD
    unset TASKVEIL_ANDROID_KEY_PROPERTIES
    unset TASKVEIL_ANDROID_KEYSTORE_PATH
    unset TASKVEIL_ANDROID_KEYSTORE_PASSWORD
    unset TASKVEIL_ANDROID_KEY_ALIAS
    unset TASKVEIL_ANDROID_KEY_PASSWORD
    cd "$repo_root/app/android"
    ./gradlew \
        -PtaskveilStoreBuild=true \
        -PtaskveilKeyProperties="$missing_properties" \
        help
) >"$failure_log" 2>&1; then
    echo "store build unexpectedly accepted missing signing credentials" >&2
    exit 1
fi

if ! grep -F "Store release signing requires all configured values" \
    "$failure_log" >/dev/null; then
    echo "store build failed without the expected signing guard" >&2
    exit 1
fi

echo "Android release signing regression checks passed."

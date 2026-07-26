#!/bin/sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/server/src/routes"
cp "$repo_root/server/src/routes/sync.rs" "$fixture/server/src/routes/sync.rs"
cp "$repo_root/server/src/routes/realtime.rs" "$fixture/server/src/routes/realtime.rs"
mkdir -p "$fixture/server/tests"
cp "$repo_root/server/tests/billing_foundation.rs" "$fixture/server/tests/billing_foundation.rs"

check="$repo_root/tool/ci/check_authorized_sync_routes.sh"
TASKVEIL_AUTHORIZED_SYNC_ROOT="$fixture" sh "$check"

expect_failure() {
  label="$1"
  if TASKVEIL_AUTHORIZED_SYNC_ROOT="$fixture" sh "$check" >/dev/null 2>&1; then
    printf '%s\n' "authorized sync route fixture unexpectedly passed: $label" >&2
    exit 1
  fi
}

awk '
  !changed && /authorized: AuthorizedSyncRequest/ {
    sub(/authorized: AuthorizedSyncRequest/, "authorized: ()")
    changed = 1
  }
  { print }
' "$fixture/server/src/routes/sync.rs" > "$fixture/server/src/routes/sync.rs.next"
mv "$fixture/server/src/routes/sync.rs.next" "$fixture/server/src/routes/sync.rs"
expect_failure missing-extractor
cp "$repo_root/server/src/routes/sync.rs" "$fixture/server/src/routes/sync.rs"

awk '
  !changed && /authorized: AuthorizedSyncRequest/ {
    authorization = $0
    getline
    print
    print authorization
    changed = 1
    next
  }
  { print }
' "$fixture/server/src/routes/sync.rs" > "$fixture/server/src/routes/sync.rs.next"
mv "$fixture/server/src/routes/sync.rs.next" "$fixture/server/src/routes/sync.rs"
expect_failure input-before-authorization
cp "$repo_root/server/src/routes/sync.rs" "$fixture/server/src/routes/sync.rs"

awk '
  !changed && /Router::new\(\)/ {
    sub(/Router::new\(\)/, "Router::new().route(\"\\/probe\", post(unprotected_probe))")
    changed = 1
  }
  { print }
' "$fixture/server/src/routes/realtime.rs" > "$fixture/server/src/routes/realtime.rs.next"
mv "$fixture/server/src/routes/realtime.rs.next" "$fixture/server/src/routes/realtime.rs"
printf '%s\n' \
  'async fn unprotected_probe() -> axum::http::StatusCode {' \
  '    axum::http::StatusCode::NO_CONTENT' \
  '}' >> "$fixture/server/src/routes/realtime.rs"
expect_failure new-unprotected-route

printf '%s\n' 'authorized sync route fixtures passed'

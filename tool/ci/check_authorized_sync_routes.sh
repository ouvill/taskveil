#!/bin/sh
set -eu

status=0
root="${TASKVEIL_AUTHORIZED_SYNC_ROOT:-.}"

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

for source in \
  "$root/server/src/routes/sync.rs" \
  "$root/server/src/routes/realtime.rs"
do
  if [ ! -f "$source" ]; then
    fail "$source: protected route source is missing"
    continue
  fi

  method_count="$(
    grep -Eo '(get|post|put|patch|delete)\(' "$source" | wc -l | tr -d ' '
  )"
  handlers="$(
    grep -Eo '(get|post|put|patch|delete)\([a-z_][a-z0-9_]*\)' "$source" |
      sed -E 's/^[^(]+\(([^)]+)\)$/\1/'
  )"
  handler_count="$(printf '%s\n' "$handlers" | sed '/^$/d' | wc -l | tr -d ' ')"

  if [ "$method_count" -ne "$handler_count" ]; then
    fail "$source: every route method must use a named handler"
  fi

  for handler in $handlers; do
    signature="$(
      awk -v declaration="async fn $handler(" '
        !found && index($0, declaration) {
          found = 1
        }
        found {
          print
        }
        found && /\)[[:space:]]*->[[:space:]]*/ {
          exit
        }
      ' "$source"
    )"
    if [ -z "$signature" ]; then
      fail "$source: registered handler $handler has no async function declaration"
    elif ! printf '%s\n' "$signature" | grep -q 'AuthorizedSyncRequest'; then
      fail "$source: registered handler $handler bypasses AuthorizedSyncRequest"
    fi
  done

  if grep -E \
    'billing::authenticate_sync_request|bearer_token\(|require_current_protocol\(' \
    "$source" >/dev/null; then
    fail "$source: route handlers must not reimplement the shared authorization policy"
  fi
done

exit "$status"

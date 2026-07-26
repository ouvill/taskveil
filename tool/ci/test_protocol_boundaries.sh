#!/bin/sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/core/protocol" "$fixture/core/sync" "$fixture/server"
cp "$repo_root/core/protocol/Cargo.toml" "$fixture/core/protocol/Cargo.toml"
cp "$repo_root/core/sync/Cargo.toml" "$fixture/core/sync/Cargo.toml"
cp "$repo_root/server/Cargo.toml" "$fixture/server/Cargo.toml"

check="$repo_root/tool/ci/check_protocol_boundaries.sh"
TASKVEIL_PROTOCOL_BOUNDARY_ROOT="$fixture" sh "$check"

expect_failure() {
  label="$1"
  if TASKVEIL_PROTOCOL_BOUNDARY_ROOT="$fixture" sh "$check" >/dev/null 2>&1; then
    printf '%s\n' "protocol boundary fixture unexpectedly passed: $label" >&2
    exit 1
  fi
}

awk '
  /^\[dependencies\]$/ && !inserted {
    print
    print "reqwest.workspace = true"
    inserted = 1
    next
  }
  { print }
' "$fixture/core/protocol/Cargo.toml" > "$fixture/core/protocol/Cargo.toml.next"
mv "$fixture/core/protocol/Cargo.toml.next" "$fixture/core/protocol/Cargo.toml"
expect_failure protocol-http
cp "$repo_root/core/protocol/Cargo.toml" "$fixture/core/protocol/Cargo.toml"

awk '
  /^\[dependencies\]$/ && !inserted {
    print
    print "taskveil-sync.workspace = true"
    inserted = 1
    next
  }
  { print }
' "$fixture/server/Cargo.toml" > "$fixture/server/Cargo.toml.next"
mv "$fixture/server/Cargo.toml.next" "$fixture/server/Cargo.toml"
expect_failure server-sync
cp "$repo_root/server/Cargo.toml" "$fixture/server/Cargo.toml"

awk '
  /^\[dependencies\]$/ && !inserted {
    print
    print "runtime = { package = \"taskveil-sync\", path = \"../sync\" }"
    inserted = 1
    next
  }
  { print }
' "$fixture/server/Cargo.toml" > "$fixture/server/Cargo.toml.next"
mv "$fixture/server/Cargo.toml.next" "$fixture/server/Cargo.toml"
expect_failure hidden-server-sync

printf '%s\n' 'protocol boundary fixtures passed'

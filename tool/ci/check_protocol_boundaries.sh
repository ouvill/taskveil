#!/bin/sh
set -eu

status=0
root="${TASKVEIL_PROTOCOL_BOUNDARY_ROOT:-.}"

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

dependencies() {
  awk '
    /^\[dependencies\]$/ { in_dependencies = 1; next }
    /^\[/ { in_dependencies = 0 }
    in_dependencies && match($0, /^[[:space:]]*[A-Za-z0-9_-]+/) {
      dependency = substr($0, RSTART, RLENGTH)
      gsub(/[[:space:]]/, "", dependency)
      print dependency
    }
  ' "$1" | sort
}

dependency_lines() {
  awk '
    /^\[dependencies\]$/ { in_dependencies = 1; next }
    /^\[/ { in_dependencies = 0 }
    in_dependencies { print }
  ' "$1"
}

protocol_manifest="$root/core/protocol/Cargo.toml"
server_manifest="$root/server/Cargo.toml"
sync_manifest="$root/core/sync/Cargo.toml"

protocol_dependencies="$(dependencies "$protocol_manifest")"
protocol_allowed="$(printf '%s\n' serde thiserror uuid | sort)"
if [ "$protocol_dependencies" != "$protocol_allowed" ]; then
  fail "$protocol_manifest: protocol crate dependencies must be exactly serde, thiserror, and uuid"
fi
if dependency_lines "$protocol_manifest" |
  grep -E 'package[[:space:]]*=|path[[:space:]]*=' >/dev/null; then
  fail "$protocol_manifest: dependency aliases and path dependencies are forbidden"
fi

server_taskveil_dependencies="$(
  dependencies "$server_manifest" | grep '^taskveil-' || true
)"
server_allowed="$(printf '%s\n' taskveil-crypto taskveil-protocol | sort)"
if [ "$server_taskveil_dependencies" != "$server_allowed" ]; then
  fail "$server_manifest: production server may depend only on taskveil-crypto and taskveil-protocol"
fi
if dependency_lines "$server_manifest" |
  grep -E 'package[[:space:]]*=[[:space:]]*"taskveil-(client|domain|storage|sync)"|path[[:space:]]*=[[:space:]]*"[^"]*core/(client|domain|storage|sync)"' >/dev/null; then
  fail "$server_manifest: lower client/runtime crates must not be hidden behind dependency aliases"
fi

sync_taskveil_dependencies="$(
  dependencies "$sync_manifest" | grep '^taskveil-' || true
)"
sync_required="$(printf '%s\n' taskveil-crypto taskveil-domain taskveil-protocol | sort)"
if [ "$sync_taskveil_dependencies" != "$sync_required" ]; then
  fail "$sync_manifest: sync must depend exactly on taskveil-crypto, taskveil-domain, and taskveil-protocol among Taskveil crates"
fi

exit "$status"

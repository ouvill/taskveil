#!/bin/sh
set -eu

status=0
root="${TASKVEIL_BOUNDARY_ROOT:-.}"

fail() {
  printf '%s\n' "$1" >&2
  status=1
}

for required_file in \
  app/rust/Cargo.toml \
  app/rust/src/api.rs \
  app/rust/src/api/conversions.rs \
  cli/Cargo.toml \
  mcp-server/Cargo.toml; do
  if [ ! -f "$root/$required_file" ]; then
    fail "$root/$required_file: required boundary input is missing"
  fi
done
if [ ! -d "$root/app/lib/src" ]; then
  fail "$root/app/lib/src: required Dart boundary source is missing"
fi
if [ "$status" -ne 0 ]; then
  exit "$status"
fi

for relative_manifest in cli/Cargo.toml mcp-server/Cargo.toml; do
  manifest="$root/$relative_manifest"
  if grep -E '^[[:space:]]*taskveil-' "$manifest" |
    grep -Ev '^[[:space:]]*taskveil-client([.]workspace)?[[:space:]]*=' >/dev/null ||
    grep -En 'package[[:space:]]*=[[:space:]]*"taskveil-(crypto|domain|storage|sync)"|path[[:space:]]*=[[:space:]]*"[^"]*core/(crypto|domain|storage|sync)"' "$manifest" >/dev/null; then
    fail "$manifest: frontend adapter must depend on taskveil-client, not lower Taskveil crates"
  fi
done

app_dependencies="$(
  awk '
    /^\[dependencies\]$/ { in_dependencies = 1; next }
    /^\[/ { in_dependencies = 0 }
    in_dependencies && match($0, /^[[:space:]]*[A-Za-z0-9_-]+/) {
      dependency = substr($0, RSTART, RLENGTH)
      gsub(/[[:space:]]/, "", dependency)
      print dependency
    }
  ' "$root/app/rust/Cargo.toml" | sort
)"
if [ "$app_dependencies" != "$(printf '%s\n' flutter_rust_bridge taskveil-client | sort)" ]; then
  fail 'app/rust/Cargo.toml: only flutter_rust_bridge and taskveil-client are allowed dependencies'
fi
if grep -En 'package[[:space:]]*=[[:space:]]*"taskveil-(crypto|domain|storage|sync)"|path[[:space:]]*=[[:space:]]*"[^"]*core/(crypto|domain|storage|sync)"' "$root/app/rust/Cargo.toml" >/dev/null; then
  fail 'app/rust/Cargo.toml: lower Taskveil crates must not be hidden behind dependency aliases'
fi

for legacy_source in "$root/app/rust/src/support.rs" "$root/app/rust/src/sync_store.rs" \
  "$root/app/rust/src/profile_handle.rs"; do
  if [ -e "$legacy_source" ]; then
    fail "$legacy_source: legacy bridge implementation must be removed"
  fi
done

if find "$root/app/rust/src" -type f -name '*.rs' ! -name 'frb_generated.rs' \
  -exec grep -En 'taskveil_(crypto|domain|storage|sync)|open_encrypted|Sqlite[A-Za-z0-9_]*|[A-Za-z0-9_]*Repository|AccountClient|LocalSyncStore|LocalMutationContext|load_or_create_device_key|tokio|zeroize' {} + \
  >/dev/null; then
  fail 'app/rust/src: handwritten bridge code must not reference lower-layer implementation'
fi

if find "$root/app/rust/src" -type f -name '*.rs' ! -name 'frb_generated.rs' ! -name 'client_handle.rs' \
  -exec grep -En 'OnceLock' {} + >/dev/null; then
  fail 'app/rust/src: process-global TaskveilClient handle is only allowed in client_handle.rs'
fi

if grep -En 'pub[[:space:]]+fn[[:space:]]+(get_setting|set_setting)[[:space:]]*\(' \
  "$root/app/rust/src/api.rs" >/dev/null; then
  fail 'app/rust/src/api.rs: raw string-key settings APIs must not cross the frontend boundary'
fi

if grep -En 'Result<[^;{]*,[[:space:]]*String[[:space:]]*>|map_err\([^)]*to_string' \
  "$root/app/rust/src/api.rs" "$root/app/rust/src/api/conversions.rs" >/dev/null; then
  fail 'app/rust/src: public FRB failures must use BridgeErrorDto, not internal strings'
fi

if grep -En 'pub[[:space:]]+fn[[:space:]]+(greet|create_draft_task)[[:space:]]*\(' \
  "$root/app/rust/src/api.rs" >/dev/null; then
  fail 'app/rust/src/api.rs: toy functions must not be exposed in the production FRB surface'
fi

if grep -ERin --include='*.dart' \
  '(^|[^A-Za-z0-9_])(error|_error)[.]toString[(][)]' "$root/app/lib" >/dev/null; then
  fail 'app/lib: user-facing failures must localize BridgeErrorDto codes, not raw error strings'
fi

if find "$root/app/lib/src" \
  -type d -path '*/generated' -prune -o \
  -type f -name '*.dart' \
  -exec grep -En '[$][{]?(_?error|exception)([}]|[^A-Za-z0-9_])' {} + \
  >/dev/null; then
  fail 'app/lib/src: raw error interpolation must not reach user-facing strings'
fi

if find "$root" -type d \( -name .git -o -name target -o -name build \) -prune -o \
  -type f -name 'Cargo.toml' -exec grep -En '^name[[:space:]]*=[[:space:]]*"core"' {} + \
  >/dev/null; then
  fail 'Cargo manifest: bare core package/lib name is forbidden'
fi

if find "$root" -type d \( -name .git -o -name target -o -name build \) -prune -o \
  -type f -name 'Cargo.toml' -exec grep -En '^[[:space:]]*core([.]workspace)?[[:space:]]*=' {} + \
  >/dev/null; then
  fail 'Cargo manifest: core dependency alias is forbidden'
fi

if [ -d "$root/app/lib" ] && grep -ERin --include='*.dart' \
  '^[[:space:]]*import[[:space:]].*(design_lab|visual_qa|fake_bridge_service)' \
  "$root/app/lib" >/dev/null; then
  fail 'app/lib: production code must not import Design Lab, visual QA, or fake bridge sources'
fi

exit "$status"

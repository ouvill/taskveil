#!/bin/sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT

mkdir -p "$fixture/app/rust/src" "$fixture/app/lib" "$fixture/cli" "$fixture/mcp-server"
cp "$repo_root/app/rust/Cargo.toml" "$fixture/app/rust/Cargo.toml"
cp "$repo_root/app/rust/src/api.rs" "$repo_root/app/rust/src/lib.rs" \
  "$repo_root/app/rust/src/client_handle.rs" "$fixture/app/rust/src/"
mkdir -p "$fixture/app/rust/src/api"
cp "$repo_root/app/rust/src/api/conversions.rs" \
  "$fixture/app/rust/src/api/conversions.rs"
cp -R "$repo_root/app/lib/src" "$fixture/app/lib/"
cp "$repo_root/cli/Cargo.toml" "$fixture/cli/Cargo.toml"
cp "$repo_root/mcp-server/Cargo.toml" "$fixture/mcp-server/Cargo.toml"

check="$repo_root/app/tool/check_client_boundaries.sh"
TASKVEIL_BOUNDARY_ROOT="$fixture" sh "$check"

expect_failure() {
  label="$1"
  if TASKVEIL_BOUNDARY_ROOT="$fixture" sh "$check" >/dev/null 2>&1; then
    printf '%s\n' "boundary fixture unexpectedly passed: $label" >&2
    exit 1
  fi
}

printf '%s\n' 'use taskveil_storage as forbidden;' > "$fixture/app/rust/src/rogue.rs"
expect_failure lower-source
rm "$fixture/app/rust/src/rogue.rs"

printf '%s\n' 'taskveil-storage.workspace = true' >> "$fixture/cli/Cargo.toml"
expect_failure cli-lower-dependency
cp "$repo_root/cli/Cargo.toml" "$fixture/cli/Cargo.toml"

printf '%s\n' 'rogue = { package = "taskveil-storage", path = "../../../core/storage" }' >> \
  "$fixture/app/rust/Cargo.toml"
expect_failure hidden-app-alias
cp "$repo_root/app/rust/Cargo.toml" "$fixture/app/rust/Cargo.toml"

printf '%s\n' '// legacy' > "$fixture/app/rust/src/support.rs"
expect_failure legacy-source
rm "$fixture/app/rust/src/support.rs"

printf '%s\n' '// superseded handle name' > "$fixture/app/rust/src/profile_handle.rs"
expect_failure legacy-profile-handle
rm "$fixture/app/rust/src/profile_handle.rs"

printf '%s\n' 'use std::sync::OnceLock;' 'static ROGUE: OnceLock<()> = OnceLock::new();' > \
  "$fixture/app/rust/src/rogue_handle.rs"
expect_failure rogue-process-handle
rm "$fixture/app/rust/src/rogue_handle.rs"

printf '%s\n' 'pub fn set_setting(key: String, value: String) {}' >> \
  "$fixture/app/rust/src/api.rs"
expect_failure raw-settings-api
cp "$repo_root/app/rust/src/api.rs" "$fixture/app/rust/src/api.rs"

printf '%s\n' 'pub fn rogue_string_error() -> Result<(), String> { Ok(()) }' >> \
  "$fixture/app/rust/src/api.rs"
expect_failure raw-string-result
cp "$repo_root/app/rust/src/api.rs" "$fixture/app/rust/src/api.rs"

printf '%s\n' 'pub fn greet() -> Result<(), BridgeErrorDto> { Ok(()) }' >> \
  "$fixture/app/rust/src/api.rs"
expect_failure toy-api
cp "$repo_root/app/rust/src/api.rs" "$fixture/app/rust/src/api.rs"

printf '%s\n' "String leak(Object error) => '\$error';" > \
  "$fixture/app/lib/src/rogue_error_interpolation.dart"
expect_failure raw-dart-interpolation
rm "$fixture/app/lib/src/rogue_error_interpolation.dart"

rm "$fixture/app/rust/src/api/conversions.rs"
expect_failure missing-boundary-input
cp "$repo_root/app/rust/src/api/conversions.rs" \
  "$fixture/app/rust/src/api/conversions.rs"

printf '%s\n' "import '../../tool/design_lab.dart';" > \
  "$fixture/app/lib/rogue_design_import.dart"
expect_failure production-design-lab-import
rm "$fixture/app/lib/rogue_design_import.dart"

mkdir -p "$fixture/rogue"
printf '%s\n' '[package]' 'name = "core"' > "$fixture/rogue/Cargo.toml"
expect_failure bare-core

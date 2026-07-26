#!/bin/sh
set -eu

cli_binary="${1:-target/release/taskveil}"
mcp_binary="${2:-target/release/taskveil-mcp-server}"
temporary_directory="$(mktemp -d)"
trap 'rm -rf "$temporary_directory"' EXIT HUP INT TERM

check_failure() {
  name="$1"
  expected_stderr="$2"
  shift 2
  stdout_file="$temporary_directory/$name.stdout"
  stderr_file="$temporary_directory/$name.stderr"

  set +e
  "$@" >"$stdout_file" 2>"$stderr_file"
  status=$?
  set -e

  if [ "$status" -ne 1 ]; then
    echo "$name must exit with status 1, got $status" >&2
    exit 1
  fi
  if [ -s "$stdout_file" ]; then
    echo "$name must not write to stdout" >&2
    exit 1
  fi
  if [ "$(cat "$stderr_file")" != "$expected_stderr" ]; then
    echo "$name emitted an unexpected diagnostic" >&2
    exit 1
  fi
}

cli_diagnostic="taskveil: operational commands are unavailable in this build"
check_failure cli-add "$cli_diagnostic" "$cli_binary" add "do-not-echo-title"
check_failure cli-list "$cli_diagnostic" "$cli_binary" list
check_failure cli-done "$cli_diagnostic" "$cli_binary" done "do-not-echo-id"
check_failure \
  mcp-stub \
  "taskveil-mcp-server: MCP transport is unavailable in this build" \
  "$mcp_binary"

if ! "$cli_binary" --help >"$temporary_directory/help.stdout" 2>"$temporary_directory/help.stderr"; then
  echo "taskveil --help must succeed" >&2
  exit 1
fi
if ! "$cli_binary" --version >"$temporary_directory/version.stdout" 2>"$temporary_directory/version.stderr"; then
  echo "taskveil --version must succeed" >&2
  exit 1
fi
if [ ! -s "$temporary_directory/help.stdout" ] || [ ! -s "$temporary_directory/version.stdout" ]; then
  echo "taskveil help and version output must not be empty" >&2
  exit 1
fi

echo "CLI and MCP release stubs fail closed."

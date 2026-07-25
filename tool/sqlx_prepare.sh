#!/usr/bin/env bash
set -euo pipefail

# Generates or verifies SQLx offline metadata against a fresh, fully migrated
# Postgres database. The temporary container is stopped and removed on exit.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POSTGRES_IMAGE="${TASKVEIL_SQLX_POSTGRES_IMAGE:-postgres:16-alpine}"
CONTAINER_NAME="taskveil-sqlx-prepare-$$"
CONTAINER_ID=""
POSTGRES_USER="taskveil_sqlx"
POSTGRES_PASSWORD="taskveil_sqlx_local"
POSTGRES_DB="taskveil_sqlx"
MODE="${1:-prepare}"

case "$MODE" in
  prepare)
    CHECK_MODE=false
    ;;
  --check)
    CHECK_MODE=true
    ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac

command -v docker >/dev/null 2>&1 || {
  echo "docker is required" >&2
  exit 1
}

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required" >&2
  exit 1
}

SQLX_CLI_OUTPUT="$(cargo sqlx --version 2>/dev/null || true)"
SQLX_CLI_VERSION="${SQLX_CLI_OUTPUT##* }"
if [ "$SQLX_CLI_VERSION" != "0.9.0" ]; then
  echo "sqlx-cli 0.9.0 with Postgres support is required" >&2
  echo "install it with: cargo install sqlx-cli --version 0.9.0 --locked --no-default-features --features postgres,rustls" >&2
  exit 1
fi

cleanup() {
  if [ -n "$CONTAINER_ID" ]; then
    docker stop "$CONTAINER_ID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

cd "$ROOT_DIR"
CONTAINER_ID="$(docker run --rm -d \
  --name "$CONTAINER_NAME" \
  -p 127.0.0.1::5432 \
  -e "POSTGRES_USER=$POSTGRES_USER" \
  -e "POSTGRES_PASSWORD=$POSTGRES_PASSWORD" \
  -e "POSTGRES_DB=$POSTGRES_DB" \
  "$POSTGRES_IMAGE")"

POSTGRES_PORT="$(
  docker inspect \
    -f '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
    "$CONTAINER_ID"
)"
export DATABASE_URL="postgres://${POSTGRES_USER}:${POSTGRES_PASSWORD}@127.0.0.1:${POSTGRES_PORT}/${POSTGRES_DB}?sslmode=disable"
unset SQLX_OFFLINE

DATABASE_READY=false
for _ in $(seq 1 60); do
  # The official Postgres image starts a temporary Unix-socket-only server
  # while initializing the data directory. An in-container pg_isready can
  # observe that server before the published TCP endpoint used by SQLx exists.
  if cargo sqlx migrate info --source server/migrations >/dev/null 2>&1; then
    DATABASE_READY=true
    break
  fi

  CONTAINER_RUNNING="$(
    docker inspect -f '{{.State.Running}}' "$CONTAINER_ID" 2>/dev/null || true
  )"
  if [ "$CONTAINER_RUNNING" != "true" ]; then
    break
  fi
  sleep 1
done

if [ "$DATABASE_READY" != "true" ]; then
  echo "Postgres did not become reachable through $DATABASE_URL" >&2
  docker inspect \
    -f 'container status={{.State.Status}} exit_code={{.State.ExitCode}} error={{.State.Error}}' \
    "$CONTAINER_ID" >&2 || true
  docker logs "$CONTAINER_ID" >&2 || true
  echo "SQLx connection check:" >&2
  cargo sqlx migrate info --source server/migrations >&2 || true
  exit 1
fi

cargo sqlx migrate run --source server/migrations
if [ "$CHECK_MODE" = true ]; then
  cargo sqlx prepare --check --workspace -- --all-targets
else
  cargo sqlx prepare --workspace -- --all-targets
fi

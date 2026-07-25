#!/usr/bin/env bash
set -euo pipefail

# Starts the local Taskveil development server stack.
#
# What it does:
#   1. Reuses or creates Docker container "taskveil-dev-postgres".
#   2. Publishes Postgres on the first free localhost port from 5432 upward.
#   3. Applies server/migrations/*.sql before starting the Rust server.
#   4. Runs `cargo run -p taskveil-server --bin taskveil-server` with the runtime URL.
#
# Stop the Rust server with Ctrl-C.
# Keep the database for the next run, or stop it explicitly with:
#   docker stop taskveil-dev-postgres
# To delete the dev database completely:
#   docker rm -f taskveil-dev-postgres

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER_NAME="${TASKVEIL_DEV_POSTGRES_CONTAINER:-taskveil-dev-postgres}"
POSTGRES_IMAGE="${TASKVEIL_DEV_POSTGRES_IMAGE:-postgres:16-alpine}"
POSTGRES_USER="${TASKVEIL_DEV_POSTGRES_USER:-taskveil}"
POSTGRES_PASSWORD="${TASKVEIL_DEV_POSTGRES_PASSWORD:-taskveil}"
POSTGRES_DB="${TASKVEIL_DEV_POSTGRES_DB:-taskveil_dev}"
RUNTIME_USER="taskveil_runtime"
RUNTIME_PASSWORD="taskveil_runtime"
SERVER_PORT="${PORT:-8080}"

cd "$ROOT_DIR"

command -v docker >/dev/null 2>&1 || {
  echo "docker is required" >&2
  exit 1
}

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required" >&2
  exit 1
}

port_in_use() {
  local port="$1"
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1
  else
    nc -z 127.0.0.1 "$port" >/dev/null 2>&1
  fi
}

find_free_port() {
  local port="${1:-5432}"
  while port_in_use "$port"; do
    port=$((port + 1))
  done
  printf '%s\n' "$port"
}

container_exists() {
  docker container inspect "$CONTAINER_NAME" >/dev/null 2>&1
}

container_running() {
  [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER_NAME" 2>/dev/null || true)" = "true" ]
}

postgres_host_port() {
  docker inspect -f '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "$CONTAINER_NAME" 2>/dev/null || true
}

if container_exists; then
  if ! container_running; then
    echo "Starting existing Postgres container: $CONTAINER_NAME"
    docker start "$CONTAINER_NAME" >/dev/null
  else
    echo "Reusing running Postgres container: $CONTAINER_NAME"
  fi
  POSTGRES_PORT="$(postgres_host_port)"
  if [ -z "$POSTGRES_PORT" ] || [ "$POSTGRES_PORT" = "<no value>" ]; then
    echo "Existing container $CONTAINER_NAME does not publish 5432/tcp to localhost." >&2
    echo "Remove it with: docker rm -f $CONTAINER_NAME" >&2
    exit 1
  fi
else
  POSTGRES_PORT="$(find_free_port 5432)"
  echo "Creating Postgres container: $CONTAINER_NAME on localhost:$POSTGRES_PORT"
  docker run -d \
    --name "$CONTAINER_NAME" \
    -e POSTGRES_USER="$POSTGRES_USER" \
    -e POSTGRES_PASSWORD="$POSTGRES_PASSWORD" \
    -e POSTGRES_DB="$POSTGRES_DB" \
    -p "127.0.0.1:${POSTGRES_PORT}:5432" \
    "$POSTGRES_IMAGE" >/dev/null
fi

echo "Waiting for Postgres readiness..."
until docker exec "$CONTAINER_NAME" pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB" >/dev/null 2>&1; do
  sleep 1
done

echo "Applying SQL migrations..."
for migration in server/migrations/*.sql; do
  echo "  $migration"
  docker exec -i "$CONTAINER_NAME" \
    psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
    < "$migration" >/dev/null
done

if ! docker exec "$CONTAINER_NAME" \
  psql -tAc "SELECT 1 FROM pg_roles WHERE rolname = '${RUNTIME_USER}'" \
    -U "$POSTGRES_USER" -d "$POSTGRES_DB" | grep -qx 1; then
  docker exec "$CONTAINER_NAME" \
    psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
    -c "CREATE ROLE ${RUNTIME_USER} LOGIN PASSWORD '${RUNTIME_PASSWORD}' NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS" \
    >/dev/null
fi
docker exec "$CONTAINER_NAME" \
  psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
  -c "ALTER ROLE ${RUNTIME_USER} LOGIN PASSWORD '${RUNTIME_PASSWORD}' NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOBYPASSRLS; GRANT taskveil_app TO ${RUNTIME_USER}" \
  >/dev/null

if port_in_use "$SERVER_PORT"; then
  echo "Port $SERVER_PORT is already in use. Stop that process before starting taskveil-server." >&2
  exit 1
fi

export DATABASE_URL="postgres://${RUNTIME_USER}:${RUNTIME_PASSWORD}@localhost:${POSTGRES_PORT}/${POSTGRES_DB}"
export PORT="$SERVER_PORT"
export TASKVEIL_AUTH_ISSUER="${TASKVEIL_AUTH_ISSUER:-http://localhost:${PORT}}"
export RUST_LOG="${RUST_LOG:-info,taskveil_server=debug}"

echo "Starting taskveil-server on http://localhost:${PORT}"
echo "DATABASE_URL=postgres://${RUNTIME_USER}:<redacted>@localhost:${POSTGRES_PORT}/${POSTGRES_DB}"
exec cargo run -p taskveil-server --bin taskveil-server

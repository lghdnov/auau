#!/usr/bin/env bash

set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
PG_CONTAINER="auau-test-pg"
DB_NAME="auau_e2e"
HEALTH_URL="http://localhost:8080/api/v1/health"
APP_LOG="${AUAU_E2E_APP_LOG:-/tmp/auau-e2e-app.log}"

log() { echo "[up.sh] $*" 1>&2; }

log "resetting database $DB_NAME"
docker exec "$PG_CONTAINER" psql -U auau -d postgres \
	-c "DROP DATABASE IF EXISTS $DB_NAME WITH (FORCE);" \
	-c "CREATE DATABASE $DB_NAME;" >/dev/null

rm -rf "$REPO/data-e2e"
log "building web/dist"
(cd "$REPO/web" && npm run build) >/dev/null

log "starting app on :8080 (лог: $APP_LOG)"
(
	cd "$REPO/app"
	DATABASE_URL="postgres://auau:auau@localhost:5432/$DB_NAME" \
	STORAGE_DIR="$REPO/data-e2e" \
	BASE_URL="http://localhost:8080" \
	OIDC_ISSUER="http://localhost" \
	OIDC_CLIENT_ID="e2e" \
	OIDC_CLIENT_SECRET="e2e" \
	SESSION_SECRET="e2e-secret" \
	MAX_UPLOAD_MB=500 \
		cargo run --features e2e-auth-bypass
) >"$APP_LOG" 2>&1 &
APP_PID=$!

cleanup() {
	kill "$APP_PID" 2>/dev/null || true
	wait "$APP_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

APP_DIED=0
for _ in $(seq 1 240); do
	if curl -sf "$HEALTH_URL" >/dev/null 2>&1; then
		log "app is healthy"
		break
	fi
	if ! kill -0 "$APP_PID" 2>/dev/null; then
		APP_DIED=1
		break
	fi
	sleep 1
done

if [ "$APP_DIED" = "1" ]; then
	log "app process exited before becoming healthy; последние строки лога:"
	tail -n 40 "$APP_LOG" 1>&2 || true
	exit 1
fi
if ! curl -sf "$HEALTH_URL" >/dev/null 2>&1; then
	log "timeout waiting for $HEALTH_URL"
	exit 1
fi

wait "$APP_PID"

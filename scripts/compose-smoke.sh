#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

services=(app postgres pocket-id)
deadline=$((SECONDS + 300))

for svc in "${services[@]}"; do
	cid="$(docker compose ps -q "$svc")"
	if [ -z "$cid" ]; then
		echo "FAIL: сервис $svc не запущен" >&2
		exit 1
	fi
	until [ "$(docker inspect -f '{{.State.Health.Status}}' "$cid")" = "healthy" ]; do
		if [ "$SECONDS" -ge "$deadline" ]; then
			echo "FAIL: $svc не стал healthy за 300 с; последние логи:" >&2
			docker compose logs --tail 20 "$svc" >&2 || true
			exit 1
		fi
		sleep 2
	done
	echo "ok: $svc healthy"
done

health="$(curl -fsS http://localhost:8080/api/v1/health)"
echo "$health" | grep -q '"ok"'
echo "ok: /api/v1/health -> $health"

code="$(curl -fsS -o /dev/null -w '%{http_code}' http://localhost:8080/)"
[ "$code" = "200" ]
echo "ok: / -> HTTP $code"

echo "OK"

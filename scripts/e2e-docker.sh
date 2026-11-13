#!/usr/bin/env bash
# Opt-in privileged Docker integration; offline contracts: python3 scripts/test_e2e_assert.py
set -euo pipefail
set +x # credentials must never be traced
umask 077
cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="$ROOT/target-linux/release/wm"
HELPER="$ROOT/scripts/e2e_assert.py"
[[ -x "$BIN" ]] || { echo 'Build target-linux/release/wm first.' >&2; exit 1; }
command -v python3 >/dev/null
SABOTAGE=${E2E_SABOTAGE:-none}
case "$SABOTAGE" in none|route|relay|policy) ;; *) echo 'Invalid E2E_SABOTAGE' >&2; exit 1;; esac
TMP=$(mktemp -d "${TMPDIR:-/tmp}/wm-e2e.XXXXXXXX")
TMP=$(cd "$TMP" && pwd -P)
RUN="wm-e2e-$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
NET="$RUN-net"; IMG="$RUN-image"
CONTROL="$RUN-control"; A1="$RUN-a1"; A2="$RUN-a2"; SITE="$RUN-site"
CREATED=(); NETWORK=0; IMAGE=0
bounded() { python3 "$HELPER" run 30 "$@"; }
fail() { echo "E2E failed: $*" >&2; return 1; }
# Only the exact random namespace and matching ownership label may be removed.
owned() { [[ "$1" == "$RUN"-* ]] && [[ "$(bounded docker inspect -f '{{ index .Config.Labels "wm.e2e.run" }}' "$1")" == "$RUN" ]]; }
cleanup() {
  local rc=$? name log
  trap - EXIT
  set +e
  if (( rc != 0 )); then
    for name in "${CREATED[@]}"; do
      if owned "$name"; then
        log="$TMP/failure.log"
        python3 "$HELPER" logs 30 docker logs --tail 80 "$name" >"$log" 2>/dev/null
        # Raw logs stay private and are destroyed; emit only known safe event counts.
        python3 - "$log" "$name" <<'PY'
import sys
from pathlib import Path
text = Path(sys.argv[1]).read_text(errors='replace')
print('sanitized failure summary ' + sys.argv[2] + ': lines=' + str(len(text.splitlines())))
for level in ('ERROR', 'WARN', 'panic'):
    print(level + '=' + str(text.count(level)))
PY
      fi
    done
  fi
  for name in "${CREATED[@]}"; do
    if owned "$name"; then bounded docker rm -f "$name" >/dev/null || rc=1; else rc=1; fi
  done
  if (( NETWORK )); then
    if [[ "$NET" == "$RUN-net" && "$(bounded docker network inspect -f '{{ index .Labels "wm.e2e.run" }}' "$NET")" == "$RUN" ]]; then
      bounded docker network rm "$NET" >/dev/null || rc=1
    else rc=1; fi
  fi
  if (( IMAGE )); then
    if [[ "$IMG" == "$RUN-image" && "$(bounded docker image inspect -f '{{ index .Config.Labels "wm.e2e.run" }}' "$IMG")" == "$RUN" ]]; then
      bounded docker image rm "$IMG" >/dev/null || rc=1
    else rc=1; fi
  fi
  # Resolve and check the exact mktemp directory before deleting any local path.
  if [[ -n "$TMP" && -d "$TMP" && "$(cd "$TMP" && pwd -P)" == "$TMP" && "${TMP##*/}" == wm-e2e.* && "$TMP" != / ]]; then
    rm -rf -- "$TMP" || rc=1
  else rc=1; fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
wait_for() {
  local label=$1; shift
  local attempt
  for ((attempt=0; attempt<60; attempt++)); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  fail "wait exhausted: $label"
}
# No shared names, volumes, host ports or pre-run deletion.
IMAGE=1
python3 "$HELPER" run 300 docker build -q --label "wm.e2e.run=$RUN" -f deploy/e2e/Dockerfile -t "$IMG" deploy/e2e >/dev/null
NETWORK=1
bounded docker network create --label "wm.e2e.run=$RUN" "$NET" >/dev/null
PASSWORD=$(python3 -c 'import secrets; print(secrets.token_hex(24))')
printf 'WM_ADMIN_PASSWORD=%s\n' "$PASSWORD" >"$TMP/control.env"
CREATED+=("$CONTROL")
bounded docker run -d --name "$CONTROL" --label "wm.e2e.run=$RUN" --network "$NET" --network-alias control \
  -p 127.0.0.1::8080 --env-file "$TMP/control.env" -v "$BIN:/usr/local/bin/wm:ro" \
  "$IMG" serve --listen 0.0.0.0:8080 --public-url http://control:8080 --data-dir /data >/dev/null
PORT=$(bounded docker port "$CONTROL" 8080/tcp); PORT=${PORT##*:}
[[ "$PORT" =~ ^[0-9]+$ ]] || fail 'invalid control port'
BASE="http://127.0.0.1:$PORT"
http() { bounded curl --silent --show-error --fail --connect-timeout 2 --max-time 10 "$@"; }
wait_for health http "$BASE/healthz"
python3 - "$PASSWORD" >"$TMP/login.json" <<'PY'
import json, sys
json.dump(dict(username='admin', password=sys.argv[1]), sys.stdout)
PY
http -c "$TMP/cookies" -H 'content-type: application/json' --data-binary "@$TMP/login.json" "$BASE/api/v1/admin/login" >"$TMP/login-response.json"
CSRF=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["csrf_token"])' "$TMP/login-response.json")
printf 'x-wm-csrf: %s\ncontent-type: application/json\n' "$CSRF" >"$TMP/headers"
admin() { http -b "$TMP/cookies" -H "@$TMP/headers" "$@"; }
join_token() {
  admin -d "{\"name\":\"e2e-$1\",\"ttl_secs\":3600,\"max_uses\":$2,\"tags\":[\"$1\"]}" "$BASE/api/v1/admin/join-tokens" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'
}
TOKEN=$(join_token mac 2); SITE_TOKEN=$(join_token site 1)
run_node() {
  local container=$1 name=$2 token=$3; shift 3
  CREATED+=("$container")
  bounded docker run -d --name "$container" --label "wm.e2e.run=$RUN" --network "$NET" \
    --privileged --device /dev/net/tun:/dev/net/tun -v "$BIN:/usr/local/bin/wm:ro" \
    "$IMG" join http://control:8080 --token "$token" --name "$name" --data-dir /data "$@" >/dev/null
}
run_node "$A1" a1 "$TOKEN" --expose 8421
run_node "$A2" a2 "$TOKEN" --force-relay --expose 8421
run_node "$SITE" site "$SITE_TOKEN"
wm() { local node=$1; shift; bounded docker exec "$node" /usr/local/bin/wm "$@" --data-dir /data; }
status() { wm "$1" status --json | python3 "$HELPER" status "$2"; }
wait_for a1-status status "$A1" a1
wait_for a2-status status "$A2" a2
wait_for site-status status "$SITE" site
# Establish an explicit disposable baseline; never depend on production defaults.
admin -X PUT -d '{"yaml":"version: 1\nrules:\n  - action: accept\n    src: [\"tag:site\", \"tag:mac\"]\n    dst: [\"tag:mac\"]\n    protocol: tcp\n    ports: [\"8421\"]\n"}' "$BASE/api/v1/admin/acl" | python3 "$HELPER" acl
for node in "$A1" "$A2"; do
  bounded docker exec "$node" python3 -c 'import pathlib,sys; pathlib.Path("/tmp/e2e-marker").write_text(sys.argv[1])' "$RUN"
  bounded docker exec -d "$node" python3 -m http.server 8421 --bind 127.0.0.1 --directory /tmp
done
paths() { wm "$SITE" peers --json | python3 "$HELPER" peers a1 direct a2 relay; }
probe() { wm "$1" probe "$2" 8421 | python3 "$HELPER" probe "$2" "$3" "$4"; }
wait_for paths paths
# Opt-in live sabotages affect only this run's disposable containers.
if [[ "$SABOTAGE" == route ]]; then
  ROUTE_IP=$(wm "$SITE" peers --json | python3 -c 'import ipaddress,json,sys; print(ipaddress.IPv4Address(next(p["overlay_ip"] for p in json.load(sys.stdin) if p["name"]=="a1")))')
  bounded docker exec "$SITE" ip route add blackhole "$ROUTE_IP/32"
fi
if [[ "$SABOTAGE" == relay ]]; then
  bounded docker network disconnect "$NET" "$A2"
fi
wait_for direct-service probe "$SITE" a1 direct connected
wait_for relay-service probe "$SITE" a2 relay connected
response() {
  local ip body
  ip=$(wm "$SITE" peers --json | python3 -c 'import ipaddress,json,sys; p=[p for p in json.load(sys.stdin) if p["name"]==sys.argv[1]]; sys.exit(1) if len(p)!=1 else print(ipaddress.IPv4Address(p[0]["overlay_ip"]))' "$1")
  body=$(bounded docker exec "$SITE" curl --fail --silent --connect-timeout 2 --max-time 5 "http://$ip:8421/e2e-marker")
  [[ "$body" == "$RUN" ]]
}
response a1 || fail 'direct HTTP response'
response a2 || fail 'relay HTTP response'
wait_for baseline-mac-access probe "$A2" a1 relay connected
# A transport failure is not an ACL timeout and cannot satisfy this test.
if [[ "$SABOTAGE" != policy ]]; then
admin -X PUT -d '{"yaml":"version: 1\nrules:\n  - action: accept\n    src: [\"tag:site\"]\n    dst: [\"tag:mac\"]\n    protocol: tcp\n    ports: [\"8421\"]\n"}' "$BASE/api/v1/admin/acl" | python3 "$HELPER" acl
fi
wait_for acl-denial probe "$A2" a1 relay timeout
probe "$SITE" a1 direct connected
probe "$SITE" a2 relay connected
# Direct transport must survive control/relay loss; an explicit path is still required.
bounded docker stop "$CONTROL" >/dev/null
probe "$SITE" a1 direct connected
printf 'E2E assertions passed (run %s).\n' "$RUN"

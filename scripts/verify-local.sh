#!/usr/bin/env bash
# Non-privileged candidate validation. No Docker, TUN or service installation.
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
ROOT="$PWD"
WITH_PANEL=0
case "${1:-}" in
  '') ;;
  --with-panel) WITH_PANEL=1; shift ;;
  *) printf 'Usage: %s [--with-panel]\n' "$0" >&2; exit 2 ;;
esac
[ "$#" -eq 0 ] || { printf 'Unexpected argument\n' >&2; exit 2; }
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-home}"
mkdir -p "$ROOT/evidence/local"
OUT=$(mktemp -d "$ROOT/evidence/local/run.XXXXXXXX")
run() {
  local label=$1; shift
  printf '\n== %s ==\n' "$label"
  if "$@" >"$OUT/$label.log" 2>&1; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s (see %s)\n' "$label" "$OUT/$label.log" >&2
    return 1
  fi
}
printf 'Evidence: %s\n' "$OUT"
# Server validation has no Node or web/dist prerequisite.
if [ "$WITH_PANEL" -eq 1 ]; then
  run web-typecheck pnpm --dir web typecheck
  run web-build pnpm --dir web build
  run panel-package python3 scripts/package_control_panel.py --output "$OUT/control_panel.tar.gz"
fi
run panel-package-tests python3 -B -m unittest discover -s scripts -p 'test_package_control_panel.py' -v
run rust-tests cargo test --workspace --offline --locked -- --test-threads=1
run e2e-syntax bash -n scripts/e2e-docker.sh
run e2e-contracts python3 scripts/test_e2e_assert.py
run e2e-contracts-optimized python3 -O scripts/test_e2e_assert.py
run candidate-build cargo build --offline --locked -p wired-magic
run local-smoke python3 -B scripts/smoke-local.py --binary "$ROOT/target/debug/wm"
run host-control-smoke python3 -B scripts/smoke-local.py --binary "$ROOT/target/debug/wm" --host-no-agent
python3 - "$ROOT" "$OUT" <<'PY'
import datetime, hashlib, json, pathlib, platform, subprocess, sys
root, out = map(pathlib.Path, sys.argv[1:])
def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()
paths = [root / 'Cargo.toml', root / 'Cargo.lock', root / 'target/debug/wm']
for directory in ('crates', 'web/src', 'web/dist', 'scripts', 'docs'):
    paths.extend(p for p in (root / directory).rglob('*')
                 if p.is_file() and '__pycache__' not in p.parts)
paths.extend(root / 'web' / name for name in ('package.json', 'pnpm-lock.yaml', 'index.html'))
files = {str(p.relative_to(root)): digest(p) for p in sorted(set(paths)) if p.is_file()}
try:
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, stderr=subprocess.DEVNULL, text=True).strip()
except subprocess.CalledProcessError:
    commit = None
report = {
    'status': 'passed-local-only', 'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'platform': platform.platform(), 'git_commit': commit,
    'uncommitted_changes_possible': True, 'sha256': files,
    'not_validated': ['browser DOM', 'TUN/dataplane', 'Docker E2E and live sabotages',
                      'systemd/launchd reboot', 'real NAT/UDP games', 'external Seance integration'],
}
(out / 'manifest.json').write_text(json.dumps(report, indent=2) + '\n')
PY
printf '\nPASS local candidate; NOT field/production acceptance. Evidence: %s\n' "$OUT"

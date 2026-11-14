#!/bin/sh
# Hook de ativação da extensão pública `debug_gui`.
#
# Grava /usr/local/bin/wm-debug-gui (painel web) e /usr/local/bin/debug-discover
# (descoberta local), ambos apontando para o binário do agente. O `--data-dir`
# é embutido para a descoberta encontrar <data_dir>/extensions.
set -eu

find_wm() {
  for candidate in "${WM_BIN:-}" "${WM_AGENT_BIN:-}" /usr/local/bin/wm /opt/homebrew/bin/wm /usr/bin/wm /snap/bin/wm; do
    [ -n "$candidate" ] || continue
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  if command -v wm >/dev/null 2>&1; then
    found="$(command -v wm)"
    case "$found" in
      /*)
        printf '%s\n' "$found"
        return 0
        ;;
    esac
  fi
  for home in /Users/* /home/*; do
    if [ -x "$home/.cargo/bin/wm" ]; then
      printf '%s\n' "$home/.cargo/bin/wm"
      return 0
    fi
  done
  return 1
}

wm="$(find_wm || true)"
if [ -z "$wm" ]; then
  echo "debug_gui: binário 'wm' não encontrado no nó." >&2
  exit 1
fi

case "$wm" in
  /*) ;;
  *)
    echo "debug_gui: caminho do wm não é absoluto: $wm" >&2
    exit 1
    ;;
esac
case "$wm" in
  *[!A-Za-z0-9._/+@-]*)
    echo "debug_gui: caminho do wm incomum: $wm" >&2
    exit 1
    ;;
esac

node="${WM_NODE_NAME:-}"
case "$node" in
  *[!A-Za-z0-9._-]*) node="" ;;
esac
data_dir="${WM_DATA_DIR:-}"
case "$data_dir" in
  *'"'*|*'$'*|*'`'*) data_dir="" ;;
esac

install -d /usr/local/bin

write_gui() {
  tmp="$(mktemp /usr/local/bin/wm-debug-gui.XXXXXX)"
  {
    printf '#!/bin/sh\n'
    printf 'exec "%s" debug_gui' "$wm"
    if [ -n "$data_dir" ]; then printf ' --data-dir "%s"' "$data_dir"; fi
    if [ -n "$node" ]; then printf ' --node "%s"' "$node"; fi
    printf ' "$@"\n'
  } > "$tmp"
  chmod 0755 "$tmp"
  mv -f "$tmp" /usr/local/bin/wm-debug-gui
}

write_discover() {
  tmp="$(mktemp /usr/local/bin/debug-discover.XXXXXX)"
  {
    printf '#!/bin/sh\n'
    printf 'exec "%s" debug' "$wm"
    if [ -n "$data_dir" ]; then printf ' --data-dir "%s"' "$data_dir"; fi
    printf ' discover "$@"\n'
  } > "$tmp"
  chmod 0755 "$tmp"
  mv -f "$tmp" /usr/local/bin/debug-discover
}

trap 'rm -f /usr/local/bin/wm-debug-gui.* /usr/local/bin/debug-discover.* 2>/dev/null || true' EXIT INT TERM
write_gui
write_discover
trap - EXIT INT TERM

label="${node:-nó}"
echo "debug_gui: ativo em http://127.0.0.1:8791 (share debug-gui-${label})"
echo "debug_gui: atalho de descoberta em /usr/local/bin/debug-discover"

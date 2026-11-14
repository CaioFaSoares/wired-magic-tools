#!/bin/sh
# Hook de ativação da extensão pública `debug`.
#
# Grava o wrapper /usr/local/bin/wm-debug apontando para o binário do agente e
# embute o nome atual do nó (o sidecar `exec` não herda WM_NODE_NAME). O
# `default_config` do manifesto inicia esse wrapper na porta 127.0.0.1:8791.
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
  echo "debug: binário 'wm' não encontrado no nó." >&2
  exit 1
fi

case "$wm" in
  /*) ;;
  *)
    echo "debug: caminho do wm não é absoluto: $wm" >&2
    exit 1
    ;;
esac
case "$wm" in
  *[!A-Za-z0-9._/+@-]*)
    echo "debug: caminho do wm incomum: $wm" >&2
    exit 1
    ;;
esac

node="${WM_NODE_NAME:-}"
case "$node" in
  *[!A-Za-z0-9._-]*) node="" ;;
esac

install -d /usr/local/bin
tmp="$(mktemp /usr/local/bin/wm-debug.XXXXXX)"
trap 'rm -f "$tmp"' EXIT INT TERM
{
  printf '#!/bin/sh\n'
  printf 'exec "%s" debug' "$wm"
  if [ -n "$node" ]; then
    printf ' --node "%s"' "$node"
  fi
  printf ' "$@"\n'
} > "$tmp"
chmod 0755 "$tmp"
mv -f "$tmp" /usr/local/bin/wm-debug
trap - EXIT INT TERM

label="${node:-nó}"
echo "debug: ativo em http://127.0.0.1:8791 (share debug-${label})"

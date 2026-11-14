#!/bin/sh
# Hook de instalação da extensão pública `debug_gui`.
#
# Não baixa nada: o painel é o subcomando `wm debug_gui` e o atalho de
# descoberta é `wm debug discover`. O hook só confirma o binário antes de a
# ativação criar /usr/local/bin/wm-debug-gui e /usr/local/bin/debug-discover.
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

if ! "$wm" debug_gui --help >/dev/null 2>&1; then
  echo "debug_gui: '$wm' não expõe o subcomando 'debug_gui'; atualize o wired-magic." >&2
  exit 1
fi
if ! "$wm" debug discover --help >/dev/null 2>&1; then
  echo "debug_gui: '$wm' não expõe 'debug discover'; atualize o wired-magic." >&2
  exit 1
fi

install -d /usr/local/bin
echo "debug_gui: wm encontrado em $wm"

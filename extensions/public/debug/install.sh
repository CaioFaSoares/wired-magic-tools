#!/bin/sh
# Hook de instalação da extensão pública `debug`.
#
# Não baixa nada: o servidor de debug é o subcomando `wm debug`, já presente no
# binário que roda o agente. O hook só confirma que o binário existe e suporta
# o subcomando antes de a ativação criar o wrapper /usr/local/bin/wm-debug.
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
  echo "debug: instale o wm em /usr/local/bin/wm (ou defina WM_BIN) e ative de novo." >&2
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

if ! "$wm" debug --help >/dev/null 2>&1; then
  echo "debug: '$wm' não expõe o subcomando 'debug'; atualize o wired-magic." >&2
  exit 1
fi

install -d /usr/local/bin
echo "debug: wm encontrado em $wm"

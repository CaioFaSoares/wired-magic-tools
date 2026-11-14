#!/bin/sh
# Hook de instalação da extensão pública `chat`.
#
# Não baixa nada: o servidor de chat é o subcomando `wm chat`, já presente no
# binário que roda o agente. O hook só confirma que o binário existe e suporta
# o subcomando antes de a ativação criar o wrapper /usr/local/bin/wm-chat.
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
  echo "chat: binário 'wm' não encontrado no nó." >&2
  echo "chat: instale o wm em /usr/local/bin/wm (ou defina WM_BIN) e ative de novo." >&2
  exit 1
fi

case "$wm" in
  /*) ;;
  *)
    echo "chat: caminho do wm não é absoluto: $wm" >&2
    exit 1
    ;;
esac
case "$wm" in
  *[!A-Za-z0-9._/+@-]*)
    echo "chat: caminho do wm incomum: $wm" >&2
    exit 1
    ;;
esac

if ! "$wm" chat --help >/dev/null 2>&1; then
  echo "chat: '$wm' não expõe o subcomando 'chat'; atualize o wired-magic." >&2
  exit 1
fi

install -d /usr/local/bin
echo "chat: wm encontrado em $wm"

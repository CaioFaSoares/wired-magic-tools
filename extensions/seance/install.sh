#!/bin/sh
# Hook de instalação do Seance. Roda uma vez por nó/versão, como root.
# Defina SEANCE_AGENT_URL em [hooks.install.env] no manifesto ou coloque
# seance-agent no PATH antes de ativar.
set -eu
mkdir -p /usr/local/bin

if command -v seance-agent >/dev/null 2>&1; then
  found="$(command -v seance-agent)"
  # O default_config usa caminho absoluto; garanta que exista mesmo quando o
  # binário veio de outro diretório no PATH (launchd costuma ter PATH mínimo).
  if [ "$found" != "/usr/local/bin/seance-agent" ]; then
    install -m 0755 "$found" /usr/local/bin/seance-agent
    echo "seance-agent copiado de $found para /usr/local/bin/seance-agent"
  else
    echo "seance-agent já instalado: $found"
  fi
  exit 0
fi

if [ -z "${SEANCE_AGENT_URL:-}" ]; then
  echo "seance-agent não encontrado e SEANCE_AGENT_URL não definido." >&2
  echo "Coloque o binário em /usr/local/bin/seance-agent ou configure" >&2
  echo "[hooks.install.env] SEANCE_AGENT_URL no manifesto da extensão." >&2
  exit 1
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT INT TERM
echo "baixando seance-agent de $SEANCE_AGENT_URL"
curl -fsSL "$SEANCE_AGENT_URL" -o "$tmp"
install -m 0755 "$tmp" /usr/local/bin/seance-agent
echo "seance-agent instalado em /usr/local/bin/seance-agent"

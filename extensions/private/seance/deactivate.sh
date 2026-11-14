#!/bin/sh
# Hook de desativação: rode aqui uma parada graciosa se o agente precisar.
# O sidecar já foi parado pelo supervisor antes deste hook.
set -eu
echo "seance: desativado no nó ${WM_NODE_NAME:-?}"

#!/bin/sh
# Hook de ativação: rode aqui migrações ou preparação de diretórios do Seance.
# O sidecar definido em default_config é iniciado depois deste hook.
set -eu
echo "seance: ativado no nó ${WM_NODE_NAME:-?} (${WM_OVERLAY_IP:-?})"

#!/bin/sh
# Hook de desativação da extensão `debug_gui`.
#
# O supervisor já parou o sidecar antes deste hook. Os wrappers permanecem em
# /usr/local/bin para a próxima ativação ser instantânea.
set -eu
echo "debug_gui: desativado no nó ${WM_NODE_NAME:-?}"

#!/bin/sh
# Hook de desativação da extensão `chat_gui`.
#
# O supervisor já parou o sidecar antes deste hook. O wrapper permanece em
# /usr/local/bin para a próxima ativação ser instantânea.
set -eu
echo "chat_gui: desativado no nó ${WM_NODE_NAME:-?}"

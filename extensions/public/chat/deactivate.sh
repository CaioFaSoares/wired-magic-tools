#!/bin/sh
# Hook de desativação da extensão `chat`.
#
# O supervisor já parou o sidecar antes deste hook. Mantemos o wrapper em
# /usr/local/bin para que a próxima ativação seja instantânea; o endereço só
# volta a responder quando a extensão for ativada de novo.
set -eu
echo "chat: desativado no nó ${WM_NODE_NAME:-?}"

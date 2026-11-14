#!/usr/bin/env bash
# debug-publish — publica uma release do wired-magic no GitHub.
#
# Modo padrão: dispara o workflow release-macos.yml e espera o resultado.
# Modo --local: delega para scripts/publish_macos.sh (build + release + tap).
#
# Nunca guarda token no repositório: usa a sessão do `gh`. Exige --yes para
# publicar; --check e --dry-run não publicam.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

TAG=""
REPO=""
REF="main"
WORKFLOW="release-macos.yml"
MODE="workflow"
WAIT=1
CREATE_TAG=0
ALLOW_DIRTY=0
CHECK=0
DRY_RUN=0
YES=0

usage() {
  cat <<'USAGE'
Uso: extensions/private/debug-publish/publish.sh [opções]

  --tag vX.Y.Z        tag estável (default: v<versão do workspace>)
  --repo OWNER/REPO   repositório (default: gh repo view)
  --ref NAME          branch que contém o workflow (default: main)
  --workflow FILE     workflow de release (default: release-macos.yml)
  --local             build/publicação local via scripts/publish_macos.sh
  --create-tag        cria e envia a tag se ela ainda não existir no origin
  --no-wait           dispara o workflow e não espera
  --allow-dirty       permite árvore suja (não recomendado)
  --check             só valida tag, versão e repositório
  --dry-run           mostra os comandos sem publicar
  --yes               confirma a publicação
  -h, --help          esta ajuda

Ambiente: GH_TOKEN (opcional; `gh auth login` já basta).
USAGE
}

die() { printf 'erro: %s\n' "$*" >&2; exit 1; }
log() { printf '==> %s\n' "$*"; }
run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '+'; printf ' %q' "$@"; printf '\n'
  else
    "$@"
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?--tag exige valor}"; shift 2 ;;
    --repo) REPO="${2:?--repo exige valor}"; shift 2 ;;
    --ref) REF="${2:?--ref exige valor}"; shift 2 ;;
    --workflow) WORKFLOW="${2:?--workflow exige valor}"; shift 2 ;;
    --local) MODE="local"; shift ;;
    --create-tag) CREATE_TAG=1; shift ;;
    --no-wait) WAIT=0; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    --check) CHECK=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --yes) YES=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "opção desconhecida: $1 (use --help)" ;;
  esac
done

VERSION="$(python3 - <<'PY'
import tomllib
with open('Cargo.toml', 'rb') as f:
    print(tomllib.load(f)['workspace']['package']['version'])
PY
)"
[ -n "$VERSION" ] || die "não encontrei workspace.package.version em Cargo.toml"
[ -n "$TAG" ] || TAG="v$VERSION"
[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] \
  || die "tag inválida: $TAG (somente vMAJOR.MINOR.PATCH)"
[ "$TAG" = "v$VERSION" ] || die "a tag $TAG não corresponde à versão $VERSION do workspace"

command -v gh >/dev/null || die "gh não encontrado (instale GitHub CLI e rode 'gh auth login')"
command -v python3 >/dev/null || die "python3 não encontrado"
gh auth status >/dev/null 2>&1 || die "gh sem autenticação (rode 'gh auth login')"

if [ -z "$REPO" ]; then
  REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
fi
[ -n "$REPO" ] || die "não foi possível identificar o repositório"

if [ "$ALLOW_DIRTY" -eq 0 ] && [ "$CHECK" -eq 0 ]; then
  if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
    git status --short
    die "árvore suja: faça commit das alterações (ou use --allow-dirty)"
  fi
fi

LOCAL_TAG="$(git rev-parse -q --verify "refs/tags/$TAG" || true)"
REMOTE_TAG="$(git ls-remote --tags origin "refs/tags/$TAG" 2>/dev/null || true)"
if [ -z "$LOCAL_TAG" ] && [ -z "$REMOTE_TAG" ] && [ "$CREATE_TAG" -eq 0 ]; then
  die "a tag $TAG não existe; crie com --create-tag depois de commitar e enviar"
fi
if [ -n "$LOCAL_TAG" ] && [ -z "$REMOTE_TAG" ] && [ "$CREATE_TAG" -eq 0 ]; then
  die "a tag $TAG existe localmente mas não foi enviada; use --create-tag ou 'git push origin $TAG'"
fi
if [ -n "$REMOTE_TAG" ] && [ -z "$LOCAL_TAG" ]; then
  log "tag $TAG existe só no origin; o workflow vai usá-la"
fi

if [ "$CREATE_TAG" -eq 1 ]; then
  if [ -z "$LOCAL_TAG" ]; then
    run git tag -a "$TAG" -m "$TAG"
  fi
  if [ -z "$REMOTE_TAG" ]; then
    run git push origin "refs/tags/$TAG"
  fi
fi

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  die "a release $TAG já existe em $REPO"
fi

if [ "$CHECK" -eq 1 ]; then
  log "ok: tag $TAG, versão $VERSION, repositório $REPO"
  exit 0
fi

if [ "$YES" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
  die "publicar release é irreversível; repita com --yes (ou use --check/--dry-run)"
fi

if [ "$MODE" = "local" ]; then
  run scripts/publish_macos.sh --tag "$TAG"
  exit 0
fi

log "disparando $WORKFLOW em $REPO (ref $REF, tag $TAG)"
run gh workflow run "$WORKFLOW" --repo "$REPO" --ref "$REF" -f "tag=$TAG"

if [ "$DRY_RUN" -eq 1 ]; then
  log "dry-run: workflow não foi disparado"
  exit 0
fi
if [ "$WAIT" -eq 0 ]; then
  log "workflow disparado; acompanhe com: gh run list --repo $REPO --workflow $WORKFLOW"
  exit 0
fi

RUN_ID=""
for _ in $(seq 1 30); do
  RUN_ID="$(gh run list --repo "$REPO" --workflow "$WORKFLOW" --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
  [ -n "$RUN_ID" ] && break
  sleep 2
done
[ -n "$RUN_ID" ] || die "não encontrei o run do workflow; veja 'gh run list --repo $REPO'"

log "aguardando o run $RUN_ID"
gh run watch "$RUN_ID" --repo "$REPO" --exit-status
log "release publicada:"
gh release view "$TAG" --repo "$REPO" --json url,assets \
  --jq '"\(.url)  (\(.assets | length) assets)"'

#!/usr/bin/env bash
# Build macOS releases on this machine, publish them with `gh`, and update the
# Homebrew tap without spending GitHub Actions minutes. Run from the repo root:
#
#   scripts/publish_macos.sh --tag v0.1.0
#   scripts/publish_macos.sh --tag v0.1.0 --dry-run
#   scripts/publish_macos.sh --tag v0.1.0 --arch arm64
#   scripts/publish_macos.sh --tag v0.1.0 --from-release   # tap only
#
# The script refuses dirty tracked trees, existing releases, downgrades and
# non-stable tags. It never stores a token in the repository: `gh` uses your
# local login, and Homebrew owns download/checksum verification.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Prefer a workspace-local rustup, then the standard user install, even when
# the interactive shell's PATH does not include ~/.cargo/bin.
if [ -x "$ROOT/.cargo-home/bin/rustup" ]; then
    export RUSTUP_HOME="${RUSTUP_HOME:-$ROOT/.rustup}"
    export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-home}"
    export PATH="$ROOT/.cargo-home/bin:$PATH"
elif [ -x "$HOME/.cargo/bin/rustup" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

TAG=""
ARCH="both"
BINARIES=""
FROM_RELEASE=0
SKIP_WEB=0
NO_TAP=0
DRY_RUN=0
ALLOW_DIRTY=0
TAP_REPO="${WM_TAP_REPOSITORY:-}"
TAP_BRANCH="${WM_TAP_BRANCH:-main}"
RELEASE_TARGET="${WM_RELEASE_TARGET:-HEAD}"

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'

Options:
  --tag vX.Y.Z        release tag (default: v<workspace version>)
  --arch both|arm64|x86_64|host   architectures to build (default: both)
  --binaries DIR      use DIR/wm-arm64 and DIR/wm-x86_64 instead of building
  --from-release      download assets from an existing GitHub release; only tap
  --skip-web          reuse web/dist instead of rebuilding embedded assets
  --no-tap            publish the GitHub release but do not update the tap
  --tap-repo OWNER/homebrew-NAME   tap repository (default: owner/homebrew-tap)
  --tap-branch NAME   tap branch (default: main)
  --allow-dirty       allow tracked uncommitted changes (not recommended)
  --release-target REF  commit-ish for a new tag (default: HEAD)
  --dry-run           build and generate formula, but do not call the GitHub API
  -h, --help          show this help

Environment: WM_TAP_REPOSITORY, WM_TAP_BRANCH, WM_RELEASE_TARGET.
EOF
}

die() { printf 'erro: %s\n' "$*" >&2; exit 1; }
log() { printf '==> %s\n' "$*"; }

while [ $# -gt 0 ]; do
    case "$1" in
        --tag) TAG="${2:?--tag requires a value}"; shift 2 ;;
        --arch) ARCH="${2:?--arch requires a value}"; shift 2 ;;
        --binaries) BINARIES="${2:?--binaries requires a value}"; shift 2 ;;
        --from-release) FROM_RELEASE=1; shift ;;
        --skip-web) SKIP_WEB=1; shift ;;
        --no-tap) NO_TAP=1; shift ;;
        --tap-repo) TAP_REPO="${2:?--tap-repo requires a value}"; shift 2 ;;
        --tap-branch) TAP_BRANCH="${2:?--tap-branch requires a value}"; shift 2 ;;
        --allow-dirty) ALLOW_DIRTY=1; shift ;;
        --release-target) RELEASE_TARGET="${2:?--release-target requires a value}"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "opção desconhecida: $1 (use --help)" ;;
    esac
done

case "$ARCH" in
    both|arm64|x86_64|host) ;;
    *) die "--arch deve ser both, arm64, x86_64 ou host" ;;
esac
case "$TAP_BRANCH" in
    ''|*[!A-Za-z0-9._-]*) die "--tap-branch inválida (sem '/')" ;;
esac
[[ "$TAP_BRANCH" != -* ]] || die "--tap-branch inválida"

VERSION="$(awk '
    /^\[workspace\.package\]/ { in_section = 1; next }
    /^\[/ { in_section = 0 }
    in_section && /^version[[:space:]]*=/ {
        gsub(/[",[:space:]]/, "", $3); print $3; exit
    }
' Cargo.toml)"
[ -n "$VERSION" ] || die "não encontrei workspace.package.version em Cargo.toml"
[ -n "$TAG" ] || TAG="v$VERSION"

if [[ ! "$TAG" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    die "tag inválida: $TAG (somente vMAJOR.MINOR.PATCH estável)"
fi
if [ "$FROM_RELEASE" -eq 0 ]; then
    [ "$TAG" = "v$VERSION" ] || die "a tag $TAG não corresponde à versão $VERSION do workspace (use --from-release para publicar assets já existentes)"
fi

command -v gh >/dev/null || die "gh não encontrado (instale GitHub CLI e rode 'gh auth login')"
command -v python3 >/dev/null || die "python3 não encontrado"
gh auth status >/dev/null 2>&1 || die "gh sem autenticação (rode 'gh auth login')"
ORIGIN_REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
[ -n "$ORIGIN_REPO" ] || die "não foi possível identificar o repositório de origem"

if [ "$FROM_RELEASE" -eq 0 ] && [ "$ALLOW_DIRTY" -eq 0 ]; then
    if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
        git status --short
        die "árvore suja: faça commit das alterações (ou use --allow-dirty, não recomendado)"
    fi
elif [ "$FROM_RELEASE" -eq 0 ]; then
    log "atenção: --allow-dirty ativo; a tag ${RELEASE_TARGET} pode não representar o binário"
fi

TARGET_COMMIT=""
if [ "$FROM_RELEASE" -eq 0 ]; then
    TARGET_COMMIT="$(git rev-parse "$RELEASE_TARGET^{commit}")"
    if [ "$DRY_RUN" -eq 0 ]; then
        gh api "repos/$ORIGIN_REPO/commits/$TARGET_COMMIT" >/dev/null 2>&1 \
            || die "o commit $TARGET_COMMIT não existe em $ORIGIN_REPO; faça push antes de publicar"
    fi
fi

HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    arm64) HOST_ARCH=arm64 ;;
    x86_64) HOST_ARCH=x86_64 ;;
    *) die "arquitetura de host não suportada: $HOST_ARCH" ;;
esac
if [ "$ARCH" = host ]; then ARCH="$HOST_ARCH"; fi
if [ "$FROM_RELEASE" -eq 1 ] && [ "$ARCH" != both ]; then
    die "--from-release detecta as arquiteturas pelos assets; não combine com --arch"
fi
if [ "$FROM_RELEASE" -eq 1 ] && [ -n "$BINARIES" ]; then
    die "--from-release não combina com --binaries"
fi

ASSETS_DIR="$ROOT/target/release-assets/$TAG"
if [ "$FROM_RELEASE" -eq 0 ]; then
    case "$ASSETS_DIR" in
        "$ROOT"/target/release-assets/*) ;;
        *) die "diretório de assets inesperado: $ASSETS_DIR" ;;
    esac
    rm -rf "$ASSETS_DIR"
    mkdir -p "$ASSETS_DIR"
fi

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

base64_decode() {
    python3 -c 'import base64, sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.read()))'
}
base64_encode() {
    python3 -c 'import base64, sys; sys.stdout.write(base64.b64encode(sys.stdin.buffer.read()).decode())'
}

archs_for() {
    case "$1" in
        both) printf '%s\n' arm64 x86_64 ;;
        arm64|x86_64) printf '%s\n' "$1" ;;
        *) die "arquitetura inválida: $1" ;;
    esac
}

triple_for() {
    case "$1" in
        arm64) printf '%s\n' aarch64-apple-darwin ;;
        x86_64) printf '%s\n' x86_64-apple-darwin ;;
        *) die "arquitetura inválida: $1" ;;
    esac
}

if [ "$FROM_RELEASE" -eq 1 ]; then
    gh release view "$TAG" --repo "$ORIGIN_REPO" >/dev/null 2>&1 || die "a release $TAG não existe em $ORIGIN_REPO"
    log "baixando assets da release $TAG (sem compilar)"
    rm -rf "$ASSETS_DIR"
    mkdir -p "$ASSETS_DIR"
    gh release download "$TAG" --repo "$ORIGIN_REPO" --dir "$ASSETS_DIR" \
        --pattern 'wired-magic-*.tar.gz' --pattern 'SHA256SUMS'
    ARCHS=()
    for arch in arm64 x86_64; do
        [ -f "$ASSETS_DIR/wired-magic-$TAG-darwin-$arch.tar.gz" ] && ARCHS+=("$arch")
    done
    [ "${#ARCHS[@]}" -gt 0 ] || die "a release $TAG não tem assets darwin"
    [ -f "$ASSETS_DIR/SHA256SUMS" ] || die "a release $TAG não tem SHA256SUMS"
else
    ARCHS=()
    while IFS= read -r arch; do ARCHS+=("$arch"); done < <(archs_for "$ARCH")

    if [ -n "$BINARIES" ]; then
        [ -d "$BINARIES" ] || die "--binaries não é um diretório: $BINARIES"
    else
        command -v rustc >/dev/null || die "rustc não encontrado; instale Rust ou use --binaries"
        command -v cargo >/dev/null || die "cargo não encontrado; instale Rust ou use --binaries"
        for arch in "${ARCHS[@]}"; do
            triple="$(triple_for "$arch")"
            target_lib="$(rustc --print target-libdir --target "$triple" 2>/dev/null || true)"
            if [ ! -d "$target_lib" ] && command -v rustup >/dev/null 2>&1; then
                log "instalando target Rust $triple"
                rustup target add "$triple"
                target_lib="$(rustc --print target-libdir --target "$triple" 2>/dev/null || true)"
            fi
            [ -d "$target_lib" ] || die "target $triple não instalado. Instale com 'rustup target add $triple' ou use '--arch host' / '--binaries DIR'"
        done
        if [ "$SKIP_WEB" -eq 0 ]; then
            command -v pnpm >/dev/null || die "pnpm não encontrado (instale pnpm ou use --binaries)"
            log "compilando web embutida"
            pnpm --dir web install --frozen-lockfile
            pnpm --dir web build
        fi
        [ -f web/dist/index.html ] || die "web/dist/index.html ausente; rode sem --skip-web"
    fi

    for arch in "${ARCHS[@]}"; do
        triple="$(triple_for "$arch")"
        if [ -n "$BINARIES" ]; then
            source_bin=""
            for candidate in "wm-$arch" "wm-$triple"; do
                if [ -f "$BINARIES/$candidate" ]; then source_bin="$BINARIES/$candidate"; break; fi
            done
            [ -n "$source_bin" ] || die "não encontrei $BINARIES/wm-$arch (ou wm-$triple)"
            log "usando binário fornecido $source_bin"
        else
            target_lib="$(rustc --print target-libdir --target "$triple" 2>/dev/null || true)"
            if [ ! -d "$target_lib" ]; then
                if command -v rustup >/dev/null 2>&1; then
                    log "instalando target Rust $triple"
                    rustup target add "$triple"
                    target_lib="$(rustc --print target-libdir --target "$triple" 2>/dev/null || true)"
                fi
            fi
            [ -d "$target_lib" ] || die "target $triple não instalado. Instale com 'rustup target add $triple' ou use '--arch host' / '--binaries DIR'"
            log "compilando $triple (release)"
            # rust-embed reads web/dist at compile time; force the crate that
            # embeds it to rerun after a fresh pnpm build.
            cargo clean -p wm-control --target "$triple" >/dev/null 2>&1 || true
            cargo build --locked --release -p wired-magic --bin wm --target "$triple"
            source_bin="$ROOT/target/$triple/release/wm"
        fi

        [ -f "$source_bin" ] || die "binário ausente: $source_bin"
        stage="$ASSETS_DIR/stage-$arch"
        rm -rf "$stage"
        mkdir -p "$stage"
        cp "$source_bin" "$stage/wm"
        chmod 755 "$stage/wm"

        if [ "$arch" = "$HOST_ARCH" ]; then
            actual="$("$stage/wm" --version)"
            "$stage/wm" --help >/dev/null 2>&1 || die "smoke test --help falhou para $arch"
        elif [ "$arch" = x86_64 ] && arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
            actual="$(arch -x86_64 "$stage/wm" --version)"
        else
            log "aviso: binário $arch não executável neste Mac; pulando smoke test"
            actual="wm $VERSION"
        fi
        [ "$actual" = "wm $VERSION" ] || die "smoke test falhou para $arch: saída '$actual', esperado 'wm $VERSION'"

        COPYFILE_DISABLE=1 tar -czf "$ASSETS_DIR/wired-magic-$TAG-darwin-$arch.tar.gz" -C "$stage" wm
        rm -rf "$stage"
        log "asset criado: wired-magic-$TAG-darwin-$arch.tar.gz"
    done

    sums=""
    for arch in "${ARCHS[@]}"; do
        archive="$ASSETS_DIR/wired-magic-$TAG-darwin-$arch.tar.gz"
        sums+="$(sha256_file "$archive")  wired-magic-$TAG-darwin-$arch.tar.gz"$'\n'
    done
    printf '%s' "$sums" > "$ASSETS_DIR/SHA256SUMS"
fi

ARCH_ARG=both
if [ "${#ARCHS[@]}" -eq 1 ]; then ARCH_ARG="${ARCHS[0]}"; fi
log "gerando formula (${ARCHS[*]})"
python3 scripts/homebrew_formula.py --repository "$ORIGIN_REPO" --tag "$TAG" \
    --assets "$ASSETS_DIR" --archs "$ARCH_ARG" --output "$ASSETS_DIR/wired-magic.rb"
if command -v ruby >/dev/null 2>&1; then
    ruby -c "$ASSETS_DIR/wired-magic.rb" >/dev/null || die "formula Ruby inválida"
fi

if [ "$DRY_RUN" -eq 1 ]; then
    log "dry-run: nenhuma chamada mutável ao GitHub"
    printf 'assets: %s\n' "$ASSETS_DIR"
    printf 'release: gh release create %s --repo %s ...\n' "$TAG" "$ORIGIN_REPO"
    [ "$NO_TAP" -eq 1 ] || printf 'tap: %s (%s) → Formula/wired-magic.rb\n' "${TAP_REPO:-<owner>/homebrew-tap}" "$TAP_BRANCH"
    exit 0
fi

TAP_SHORT=""
TMP_DIR=""
cleanup() { [ -z "$TMP_DIR" ] || rm -rf "$TMP_DIR"; }
trap cleanup EXIT
update_tap() {
    local repo="$1"
    local tmp existing_sha existing_file content
    local -a args
    [[ "$repo" =~ ^[A-Za-z0-9_.-]+/homebrew-[A-Za-z0-9_.-]+$ ]] || die "tap inválido: $repo (use OWNER/homebrew-NAME)"
    gh repo view "$repo" --json name,defaultBranchRef >/dev/null 2>&1 || die "tap não encontrado ou sem acesso: $repo"
    gh api "repos/$repo/branches/$TAP_BRANCH" >/dev/null 2>&1 || die "branch $TAP_BRANCH não existe em $repo"
    TAP_SHORT="${repo#*/}"; TAP_SHORT="${TAP_SHORT#homebrew-}"

    TMP_DIR="$(mktemp -d)"
    tmp="$TMP_DIR"
    existing_sha="$(gh api "repos/$repo/contents/Formula/wired-magic.rb?ref=$TAP_BRANCH" --jq .sha 2>/dev/null || true)"
    existing_file=""
    if [ -n "$existing_sha" ]; then
        gh api "repos/$repo/contents/Formula/wired-magic.rb?ref=$TAP_BRANCH" --jq .content | base64_decode > "$tmp/existing.rb"
        existing_file="$tmp/existing.rb"
    fi
    if [ -n "$existing_file" ]; then
        python3 scripts/homebrew_formula.py --repository "$ORIGIN_REPO" --tag "$TAG" \
            --assets "$ASSETS_DIR" --archs "$ARCH_ARG" --existing "$existing_file" --output "$tmp/new.rb" \
            || die "o tap já aponta para uma versão mais nova; downgrade recusado"
    else
        cp "$ASSETS_DIR/wired-magic.rb" "$tmp/new.rb"
    fi
    if [ -n "$existing_file" ] && cmp -s "$existing_file" "$tmp/new.rb"; then
        log "tap já está em $TAG; nada a enviar"
        return 0
    fi
    content="$(base64_encode < "$tmp/new.rb")"
    args=(-X PUT "repos/$repo/contents/Formula/wired-magic.rb"
        -f "message=wired-magic $TAG" -f "content=$content" -f "branch=$TAP_BRANCH")
    [ -z "$existing_sha" ] || args+=(-f "sha=$existing_sha")
    if ! gh api "${args[@]}" --jq .commit.html_url >/dev/null; then
        die "falha ao atualizar o tap (outro commit concorrente? tente novamente)"
    fi
    gh api "repos/$repo/contents/Formula/wired-magic.rb?ref=$TAP_BRANCH" --jq .content | base64_decode > "$tmp/remote.rb"
    cmp -s "$tmp/remote.rb" "$tmp/new.rb" || die "verificação remota da formula falhou"
    log "tap atualizado: ${repo%%/*}/$TAP_SHORT/wired-magic"
}

if [ "$FROM_RELEASE" -eq 0 ] && gh release view "$TAG" --repo "$ORIGIN_REPO" >/dev/null 2>&1; then
    die "a release $TAG já existe; não sobrescrevo assets publicados"
fi
if [ "$FROM_RELEASE" -eq 0 ]; then
    release_args=(--repo "$ORIGIN_REPO" --title "wired-magic $TAG" --generate-notes)
    local_tag=0
    remote_tag=0
    if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then local_tag=1; fi
    if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then remote_tag=1; fi
    if [ "$local_tag" -eq 1 ]; then
        tag_commit="$(git rev-parse "$TAG^{commit}")"
        target_commit="$(git rev-parse "$RELEASE_TARGET^{commit}")"
        [ "$tag_commit" = "$target_commit" ] || die "a tag $TAG não aponta para $RELEASE_TARGET; recusei publicar assets de outro commit"
    fi
    if [ "$remote_tag" -eq 1 ]; then
        [ "$local_tag" -eq 1 ] || die "a tag $TAG existe no remoto mas não localmente; faça fetch e confira o commit"
        remote_tag_commit="$(git ls-remote --tags origin "refs/tags/$TAG^{}" | awk '{print $1}')"
        [ -n "$remote_tag_commit" ] || remote_tag_commit="$(git ls-remote --tags origin "refs/tags/$TAG" | awk '{print $1}')"
        [ "$remote_tag_commit" = "$TARGET_COMMIT" ] || die "a tag remota $TAG não aponta para $TARGET_COMMIT; recusei publicar assets de outro commit"
        release_args+=(--verify-tag)
    else
        release_args+=(--target "$RELEASE_TARGET")
    fi
    log "publicando release $TAG"
    gh release create "$TAG" "${release_args[@]}" \
        "$ASSETS_DIR"/*.tar.gz "$ASSETS_DIR/SHA256SUMS" "$ASSETS_DIR/wired-magic.rb"
else
    log "release $TAG já existente; atualizando somente o tap"
fi

if [ "$NO_TAP" -eq 0 ]; then
    if [ -z "$TAP_REPO" ]; then
        TAP_REPO="$(gh repo view "$ORIGIN_REPO" --json owner --jq .owner.login)/homebrew-tap"
    fi
    update_tap "$TAP_REPO"
else
    log "--no-tap: release publicada sem atualizar o tap"
fi

if [ -n "$TAP_SHORT" ]; then
    cat <<EOF

Instalação em outro Mac:
  brew tap ${TAP_REPO%%/*}/$TAP_SHORT
  brew install ${TAP_REPO%%/*}/$TAP_SHORT/wired-magic
  wm --version

A próxima abertura de 'wm tui' verifica e instala versões mais novas do tap.
EOF
fi

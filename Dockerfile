# syntax=docker/dockerfile:1

# wired-magic — imagem única do host (control plane + relay + STUN + nó gateway).
#
# Headless por padrão: nenhum Node, React ou asset do painel entra neste build.
# Painel opcional: Dockerfile.panel produz pacote separado ativável em runtime.
# O runtime é Debian slim porque o agente precisa de `ip` (iproute2)
# e de /dev/net/tun; não é uma imagem rootless.
#
# Build:
#   docker build -t wired-magic .
# Uso via compose/Coolify:
#   docker compose up -d

FROM rust:1.98-bookworm AS build

WORKDIR /src

# Separa artefatos por arquitetura; não força plataforma (Coolify ARM64/x86_64).
ARG TARGETARCH

# O servidor não depende de web/dist (nem sequer copia a árvore web).
ARG CARGO_BUILD_JOBS=1
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# wm-ext embeds built-in manifests/scripts with include_str! at compile time.
COPY extensions ./extensions

# Downloads e compilação sobrevivem à invalidação da camada por mudanças de código.
# O cache target é um mount: copie o executável para fora dele no mesmo RUN.
RUN --mount=type=cache,id=wm-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=wm-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=wm-target-rust-1.98-${TARGETARCH},target=/src/target,sharing=locked \
    cargo build --release --locked -p wired-magic \
 && install -Dm755 target/release/wm /out/wm


FROM debian:bookworm-slim AS runtime

# curl: healthcheck do compose/Coolify.
# iproute2: o agente cria a interface TUN e instala rotas com `ip`.
# ca-certificates: chamadas HTTPS de saída (control, relay, STUN).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl iproute2 \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /out/wm /usr/local/bin/wm

# O host grava control.db, chaves, agent-state.json e o socket local aqui.
RUN mkdir -p /data && chmod 700 /data

WORKDIR /data
ENV WM_DATA_DIR=/data \
    WM_LOG=info

# 8080 é consumido pelo proxy do Coolify (Traefik) na rede interna.
# 3478/udp é o STUN; 41641/udp é o endpoint WireGuard do nó gateway.
EXPOSE 8080/tcp 3478/udp 41641/udp

# `wm host` trata SIGINT (Ctrl+C) e encerra o control de forma graciosa. O
# Docker manda SIGTERM por padrão; alinhamos para o sinal que o processo trata.
STOPSIGNAL SIGINT

HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/wm"]
CMD ["host", "--listen", "0.0.0.0:8080"]

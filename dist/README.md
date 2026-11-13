# Binários de demonstração

- `wm-darwin-arm64` — macOS 14+ Apple Silicon.
- `wm-linux-arm64` — Linux aarch64 (glibc).

Builds deste workspace (perfil release, painel React embutido). Para outras arquiteturas,
compile com `cargo build --release -p wired-magic` no alvo correspondente.

Verifique com `shasum -a 256 -c SHA256SUMS`.

Uso: `./wm-darwin-arm64 --help` ou `./wm-linux-arm64 --help`.

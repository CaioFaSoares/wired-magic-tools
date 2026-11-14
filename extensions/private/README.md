# Extensões privadas

Extensões internas/do operador. Não são tratadas como exemplo público nem
precisam de documentação de catálogo; ainda usam o mesmo contrato de manifesto
e declaram `visibility = "private"`.

- `seance/` — instala e supervisiona o agente do Seance, publica o Baguette.
- `debug-publish/` — ferramenta de operador para publicar release no GitHub.
  Não é sidecar e não é semeada no data dir; execute `publish.sh` no checkout.

O `wm-flake.toml` desta pasta lista os pacotes privados para o gerenciador:

```sh
wm packages discover extensions/private
wm packages install  extensions/private --package debug-publish --yes
```

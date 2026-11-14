# Debug Publish (privada)

Ferramenta de operador para publicar uma release no GitHub. **Não é sidecar**:
não roda em nó, não tem hooks e não é semeada no data dir. O manifesto existe
para o catálogo privado e para o gerenciador de pacotes distribuir o script.

## Uso

No checkout do mantenedor, com `gh` autenticado e a tag já commitada/enviada:

```sh
extensions/private/debug-publish/publish.sh --check
extensions/private/debug-publish/publish.sh --yes
extensions/private/debug-publish/publish.sh --tag v0.1.2 --create-tag --yes
extensions/private/debug-publish/publish.sh --local --yes
```

- **Padrão:** dispara `.github/workflows/release-macos.yml` via
  `gh workflow run`, espera o run e mostra a URL da release. O workflow builda
  arm64/x86_64, valida os checksums e publica os assets.
- **`--local`:** delega para `scripts/publish_macos.sh` (build local, release e
  tap Homebrew).
- **`--check`:** valida tag/versão/repositório sem publicar.
- **`--dry-run`:** imprime os comandos, sem publicar.
- **`--create-tag`:** cria e envia a tag anotada se ela não existir no origin.
- **`--no-wait`:** dispara o workflow e retorna.

## Segurança

- Usa a sessão do `gh`; **nunca** grava token no repositório.
- Exige `--yes` para publicar; `--check`/`--dry-run` não publicam.
- Recusa tag fora de `vMAJOR.MINOR.PATCH`, versão diferente do workspace, árvore
  suja (sem `--allow-dirty`), release já existente e tag local não enviada.
- O workflow precisa existir no ref indicado (`--ref`, padrão `main`).

## Por que é privada

O script pressupõe permissão de escrita no repositório e decide o momento de
publicar. Ele não é distribuído como parte do catálogo público nem executado
pelo control plane. Para instalar via gerenciador de pacotes, trate o repositório
como fonte e selecione `--package debug-publish`; a execução continua sendo
manual.

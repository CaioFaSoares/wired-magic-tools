# macOS via Homebrew tap

## Estado atual

- Tap público: <https://github.com/CaioFaSoares/homebrew-tap>, branch `main`.
- Release `v0.1.4` publicada e `Formula/wired-magic.rb` presente no tap. `brew tap`,
  `brew info` e `brew install` foram validados com o Homebrew 7.
- O caminho recomendado compila e publica do seu Mac, sem gastar minutos de Actions e
  sem PAT/secret: o script usa o seu login local do `gh` para criar a release e
  atualizar o tap.
- O workflow `.github/workflows/release-macos.yml` é um **fallback manual**. Ele não
  roda em push de tag, justamente para que a publicação local não acorde runners macOS.

## Instalar

```sh
brew tap CaioFaSoares/tap
brew install CaioFaSoares/tap/wired-magic
wm --version
```

macOS mínimo: Sonoma 14. A formula publicada com as duas arquiteturas usa o asset
`arm64` em Apple Silicon e o `x86_64` em Intel. Uma publicação feita só com `arm64`
(ou só `x86_64`) inclui `depends_on arch:` e recusa instalação na arquitetura ausente
com mensagem do Homebrew.

## Publicar do seu Mac (recomendado)

Pré-requisitos:

- `gh auth login` com permissão de escrita no repositório de origem e no tap;
- `pnpm` para recompilar a web embutida;
- Rust com os targets `aarch64-apple-darwin` e `x86_64-apple-darwin`. O script tenta
  `rustup target add` quando o `rustup` está disponível e explica o comando quando
  não está. Ele prefere um rustup em `.cargo-home/bin` (usado nos testes locais)
  e também encontra `~/.cargo/bin` mesmo que o `PATH` do shell não o inclua.

A árvore precisa estar limpa (a tag precisa apontar para o código que gerou o
binário) e o commit precisa estar no GitHub antes da publicação real. Depois de
commitar e enviar com `git push`:

```sh
scripts/publish_macos.sh --tag v0.1.4
```

O script:

1. valida que `vX.Y.Z` é estável e igual a `workspace.package.version`;
2. recompila `web/dist` e os binários release nas duas arquiteturas;
3. executa smoke tests `--version`/`--help` no binário nativo e, via Rosetta quando
   disponível, no `x86_64`;
4. empacota apenas `wm`, calcula `SHA256SUMS` e gera `wired-magic.rb`;
5. cria a tag/release imutável com `gh release create` (nunca sobrescreve release
   existente);
6. grava `Formula/wired-magic.rb` no tap via API do GitHub, recusando downgrade e
   verificando o conteúdo remoto depois do commit.

Opções úteis:

```sh
scripts/publish_macos.sh --tag v0.1.4 --dry-run          # compila e mostra o plano
scripts/publish_macos.sh --tag v0.1.4 --arch host        # só a arquitetura deste Mac
scripts/publish_macos.sh --tag v0.1.4 --binaries ./out   # usa out/wm-arm64 e out/wm-x86_64
scripts/publish_macos.sh --tag v0.1.4 --from-release     # só atualiza o tap
scripts/publish_macos.sh --tag v0.1.4 --no-tap           # só a release
```

`--from-release` baixa os assets de uma release já publicada (por exemplo, a criada
pelo workflow manual), regenera a formula e atualiza o tap sem recompilar. É o caminho
para recuperar de uma falha do tap sem gastar Actions.

O default do tap é `OWNER/homebrew-tap`; use `--tap-repo OWNER/homebrew-NAME` ou
`WM_TAP_REPOSITORY`/`WM_TAP_BRANCH` para outro tap. O alias curto do Homebrew é
`OWNER/NAME` (para `homebrew-tap`, `OWNER/tap`).

## Workflow manual (fallback)

Em **Actions → Release macOS (manual fallback) → Run workflow**, informe uma tag já
existente e estável (`vX.Y.Z`). Crie e envie a tag antes
(`git tag vX.Y.Z && git push origin vX.Y.Z`); o workflow não a cria. Ele:

- compila as duas arquiteturas em runners macOS;
- publica a release com checksums e formula;
- **não** atualiza o tap. Depois rode
  `scripts/publish_macos.sh --tag vX.Y.Z --from-release`.

Não há secret de tap nem variável obrigatória. O workflow não aceita publicação de tag
inexistente e falha se a release já existir, preservando a imutabilidade dos assets.
Não há assinatura/notarização Apple nem atestação de proveniência nesta automação.

## Comportamento ao abrir a TUI

O aplicativo inicia a checagem em uma thread após inicializar a tela: `brew update`,
consulta JSON da fórmula e, havendo versão SemVer estável superior, executa
`brew upgrade <tap>/wired-magic`. As notificações informam a checagem, o download e a
instalação; não há modal bloqueante. Entrada fechada, saída isolada do terminal e
limites de tempo (60 s para o update, 30 s para consulta e 600 s para instalação)
mantêm a sessão utilizável em caso de falha. Uma trava por instalação evita duas TUIs
atualizando ao mesmo tempo. `brew pin` é respeitado.

Antes de chamar o Homebrew, a TUI reconcilia locks órfãos em
`/opt/homebrew/var/homebrew/locks` (remove apenas arquivos cujo `flock` está livre,
como o `brew cleanup`) e, se houver um processo Homebrew ativo, adia a checagem para
a próxima abertura em vez de esperar pelo lock e travar um `brew install` concorrente.
Depois de cada tentativa ela repete a reconciliação; locks ainda mantidos por outro
processo são preservados.

O binário atualizado vale para a próxima abertura. A TUI atual, o agente já ativo e os
serviços não são reiniciados: atualizar o pacote **não** atualiza processos em memória.
Quando desejar migrar o agente para a versão nova, reinicie-o separadamente em uma
janela adequada. `WM_NO_AUTO_UPDATE=1 wm tui` desativa a checagem. Instalações por
build local/cópia avulsa não são alteradas. Não há checagem offline nem atualização
automática enquanto a TUI estiver fechada.

A detecção exige o caminho `Cellar/wired-magic/<versão>/bin/wm`; o tap é lido de
`source.tap` em `INSTALL_RECEIPT.json` e usado como fórmula totalmente qualificada,
sem fixar o tap no Rust. Sob `sudo`, a TUI apenas orienta a atualizar como usuário
normal; nunca executa Homebrew como root.

## Problemas comuns

- **`brew install` diz que a formula não existe**: a primeira release ainda não foi
  publicada; rode `scripts/publish_macos.sh` depois do commit/push.
- **`target ... não instalado`**: instale o target com
  `rustup target add x86_64-apple-darwin` (ou use `--arch host` / `--binaries`).
- **`árvore suja`**: faça commit e `git push`; `--allow-dirty` existe só para testes e
  não deve ser usado em release.
- **`release já existe`**: assets publicados são imutáveis. Para só corrigir o tap,
  use `--from-release`.
- **`tap já aponta para uma versão mais nova`**: o script recusa downgrade; publique
  uma versão superior ou ajuste o tap manualmente com consciência.
- **`gh` sem permissão no tap**: rode `gh auth refresh -s repo` e confirme acesso de
  escrita ao repositório do tap.
- **A TUI não oferece atualização**: ela só gerencia `Cellar/wired-magic/*/bin/wm`
  instalado pelo tap; confira `WM_NO_AUTO_UPDATE`, `brew pin` e
  `brew info --json=v2 CaioFaSoares/tap/wired-magic`.

## Gerador e testes

O gerador `scripts/homebrew_formula.py` só aceita arquivos tar reais contendo apenas
`wm` executável e confere `SHA256SUMS` antes de emitir a formula. Ele suporta as duas
arquiteturas (default) ou uma só (`--archs arm64|x86_64`), gerando `depends_on arch:`
para o caso single-arch. `--existing` compara versões estáveis numericamente e recusa
downgrade. A fórmula declara `depends_on :macos` no topo e
`depends_on macos: :sonoma` dentro de `on_macos`; o Homebrew 7 rejeita a versão fora
desse bloco. O spec padrão (URL `arm64`) existe para a validação do tap em Linux, mas
o requisito macOS impede a instalação fora do macOS.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_homebrew_formula.py' -v
bash -n scripts/publish_macos.sh
```

Os testes usam arquivos temporários e hashes calculados em tempo de execução; não
publicam release nem escrevem no tap. Após a primeira release real, valide `brew
install`, `brew test` e `wm tui` em macOS arm64 e Intel. Os demos históricos de `dist/`
não são inputs da pipeline.

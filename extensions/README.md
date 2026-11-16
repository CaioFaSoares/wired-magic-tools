# Extensões do wired-magic

A arquitetura de autoria passa a ser **core → CLI → TUI → GUI opcional**.
Veja o [contrato core-first](../docs/extensions-core-first.md) para organização
por domínio, comandos não interativos, preferências e API host/nós/servidores.
Os IDs e pacotes `_gui` abaixo são mantidos por compatibilidade de implantação;
a lógica de domínio de chat/debug não depende mais dos handlers HTTP.

Esta pasta é a fonte de verdade das extensões embutidas, separadas por
**visibilidade** e por **interface**:

```text
extensions/
├── public/
│   ├── cloudflared/        # rotas Cloudflare por nó
│   ├── chat/               # chat TUI (padrão) — node_driver = none
│   ├── chat_gui/           # servidor web do chat (8790)
│   ├── debug/              # debug TUI (padrão) — node_driver = none
│   └── debug_gui/          # painel web do debug (8791) + debug-discover
└── private/
    ├── seance/             # agente do Seance e Baguette
    └── debug-publish/      # ferramenta de release no GitHub (não é sidecar)
```

O control plane semeia os manifestos em `<data_dir>/extensions/<id>/` no
primeiro boot (sem sobrescrever edições existentes). O layout de runtime é
**plano**: `public/`/`private/` organiza o repositório, não o data dir. Scripts
de hook embutidos são arquivos gerenciados e atualizados no boot. O agente
também semeia os assets embutidos no data dir do nó.

| id | visibilidade | driver | o que faz |
| --- | --- | --- | --- |
| `cloudflared` | pública | `cloudflared` | rotas de projeto para quick/named tunnel |
| `chat` | pública | `none` | TUI `wm chat`; conecta em um `chat_gui` pela malha |
| `chat_gui` | pública | `exec` | servidor web em `127.0.0.1:8790`, share `chat-gui-<nó>` |
| `debug` | pública | `none` | TUI `wm debug`; performance local + `wm debug discover` |
| `debug_gui` | pública | `exec` | painel web em `127.0.0.1:8791`, share `debug-gui-<nó>` |
| `seance` | privada | `exec` | agente do Seance, expõe 8421 e publica `baguette-<nó>` |
| `debug-publish` | privada | `none` | ferramenta de operador para publicar release no GitHub |

A classificação é declarada no manifesto (`visibility = "public" | "private"`) e
aparece no painel. É metadado de catálogo: não muda o loader nem o supervisor.
O id aceita `[a-z0-9_-]`, então `chat`/`chat_gui` e `debug`/`debug_gui`
coexistem.

Manifestos semeados antes desta classificação não são sobrescritos. Em um data
dir antigo, edite ou remova `<data_dir>/extensions/seance/extension.toml` e
recarregue os manifestos para o Seance aparecer como privado.

## TUI é o padrão; `_gui` é a interface web

| padrão (TUI) | interface web |
| --- | --- |
| `wm chat --url http://10.66.0.x:8790` | `wm chat_gui --listen 127.0.0.1:8790` |
| `wm debug` | `wm debug_gui --listen 127.0.0.1:8791` |

O `chat_gui`/`debug_gui` são sidecars ativáveis por nó; `chat`/`debug` são
pacotes TUI sem sidecar (`node_driver = "none"`). O binário `wm` é o artefato
real das TUIs.

## Ativação e pedidos do servidor

Extensões com componente no nó são **por nó** e começam desativadas. Ative na
aba **Extensões** da TUI (`wm tui`, `Enter`), no painel ou pela API:

```sh
wm extensions                         # catálogo deste nó
wm extensions enable chat_gui          # ativa no nó atual
wm extensions disable chat_gui
```

O servidor pode **solicitar** uma extensão para um nó no painel (ou por
`PUT /api/v1/admin/extensions/{id}/nodes/{node_id}/request`). A solicitação é
uma recomendação: aparece no catálogo do nó com `!` e a mensagem, mas o membro
decide ativar. Não há instalação/execução automática.

Na aba **Extensões** da TUI (`wm tui`) há dois catálogos: o do control plane,
entregue pelo agente, e o **catálogo local** lido de `<data_dir>/extensions`.
O local aparece mesmo com o agente parado, com versão, visibilidade, driver,
status (`nova`/`atualizada`/`ok`) e caminho.

## Chat

Ative `chat_gui` **no nó que hospeda a sala**; os outros nós só abrem o share
ou usam a TUI:

```sh
# servidor
sudo wm extensions enable chat_gui

# outro nó
wm open chat-gui-<nó-servidor>          # navegador
wm chat --url http://10.66.0.x:8790     # TUI
```

O sidecar escuta `127.0.0.1:8790`; o adapter publica `chat-gui-<nó>` e expõe
`8790` **somente no overlay**. Um nó com as demais portas públicas fechadas
continua acessando pelo túnel da malha. A sala não tem autenticação por padrão;
a ACL e o share são o controle de acesso. Até 500 mensagens em memória; reiniciar
o sidecar limpa. Não publique a porta no cloudflared.

## Debug e descoberta

Ative `debug_gui` para o painel web e os atalhos:

```sh
sudo wm extensions enable debug_gui
debug-discover                 # = wm debug discover
debug-discover --json
```

O sidecar escuta `127.0.0.1:8791`, publica `debug-gui-<nó>` e expõe `8791` no
overlay. Ele mostra CPU, memória, load, discos, rede, processo `wm`, top
processos e **extensões locais**, marcando novas/atualizadas a cada 5 s. O
`debug-discover` varre `<data_dir>/extensions` e grava
`.debug-discovery.json` (janela de 24 h). A TUI `wm debug` faz a mesma coleta
local sem servidor.

## Gerenciador de pacotes (git, estilo flake)

Um repositório git é uma fonte de pacotes. Qualquer subdiretório com
`extension.toml` é um pacote; `wm-flake.toml` na raiz pode listar os pacotes
explicitamente (o próprio repo tem um exemplo em [`wm-flake.toml`](../wm-flake.toml)).

```sh
wm packages discover github:owner/repo
wm packages install  github:owner/repo --package chat_gui --yes
wm packages install  github:owner/repo --ref v1.2.0 --subdir packages --all --yes
wm packages list
wm packages update
wm packages remove chat_gui --yes
```

- `--ref` aceita branch, tag ou commit; o commit resolvido vai para
  `<data_dir>/extensions/.packages.json`.
- `--subdir` limita a descoberta a uma pasta; `--package` escolhe um id e
  `--all` instala todos.
- `--yes` confirma a execução de código no nó; `--dry-run` só mostra.
- O destino é `<data_dir>/extensions/<id>/`; recarregue os manifestos no painel
  para o control plane ver o pacote.
- O gerenciador não instala dependências de outros flakes automaticamente; os
  `inputs` de um flake ainda são responsabilidade do operador.

## Debug Publish (privada)

`private/debug-publish/publish.sh` publica uma release no GitHub via
`gh workflow run` (padrão) ou `scripts/publish_macos.sh` (`--local`). É uma
ferramenta de mantenedor, não um sidecar e não é semeada no data dir. Use
`--check`/`--dry-run` antes de `--yes`. Veja
[`extensions/private/debug-publish/README.md`](private/debug-publish/README.md).

## Adicionar extensões

1. **Embutidas:** cloudflared, chat/chat_gui, debug/debug_gui e seance já
   aparecem no catálogo embutido da TUI, mesmo antes do data dir ser semeado.
2. **Local:** copie a pasta para `<data_dir>/extensions/<id>/` (o control lê no
   boot/reload) ou instale de um repositório git:
   ```sh
   wm packages install github:owner/repo --package minha_ext --yes
   ```
3. **Ative** o componente de nó na TUI (aba Extensões, `h`/`l` alterna o
   catálogo do control e o local, `Enter/t` ativa ou abre a aba TUI), no painel
   ou com `wm extensions enable <id>`.
4. O catálogo local da TUI relê `<data_dir>/extensions` a cada 5s; o catálogo do
   control só muda depois de **Recarregar manifestos** no painel.

O data dir padrão no macOS é `/usr/local/var/wired-magic` (ou `WM_DATA_DIR`);
no Linux, `/var/lib/wired-magic`. Se você só abriu `wm tui`, ainda não há data
dir semeado — por isso o catálogo embutido é mostrado primeiro.

## TUI fornecida pela extensão

Uma extensão pode declarar uma aba na TUI:

```toml
[tui]
tab = "Minha aba"
mode = "pane"           # pane | snapshot | exec
command = "{extension_dir}/panel.sh"
refresh_secs = 5
```

- `mode = "pane"` roda a extensão dentro de um **PTY embutido** no corpo da
  TUI. O wm empresta o terminal e encaminha as teclas; a extensão desenha a
  interface que precisar. `Tab`/`Shift+Tab` troca de aba; se o processo sair,
  `Enter` reinicia. É o modo usado por `chat` (`wm chat`) e `debug`.
- `mode = "snapshot"` executa o comando periodicamente e renderiza o JSON
  `wm-proto::TuiDocument` (título, seções de texto/tabela, rodapé).
- `mode = "exec"` suspende a TUI principal e abre a interface de tela cheia.
- `{wm}` vira o binário atual; `{extension_dir}` vira o diretório local da
  extensão; `{option:chave}` vira uma opção de `[tui.options]`. O comando recebe
  `WM_EXTENSION_ID`, `WM_DATA_DIR` e `WM_TUI=1` (`WM_TUI_PANE=1` no pane).
- Exemplo completo de snapshot: [`examples/extensions/tui-panel`](../examples/extensions/tui-panel).

## Criar uma extensão

- **Pública:** `public/<id>/extension.toml`, `visibility = "public"` e um
  `README.md` com contrato e segurança.
- **Privada:** `private/<id>/extension.toml`, `visibility = "private"`.

O contrato completo está em [`docs/extensions.md`](../docs/extensions.md) e
[`docs/hooks-adapters.md`](../docs/hooks-adapters.md). Extensões embutidas usam
`include_str!` em `crates/wm-ext/src/manifest.rs`; ao mover/criar uma pasta,
atualize caminhos, seeder e testes.

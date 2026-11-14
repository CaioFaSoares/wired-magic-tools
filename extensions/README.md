# Extensões do wired-magic

Esta pasta é a fonte de verdade das extensões embutidas, separadas por
**visibilidade**:

```text
extensions/
├── public/                 # distribuição aberta; catálogo documentado
│   ├── cloudflared/        # rotas por nó para quick/named tunnel
│   ├── chat/               # sala de chat web por nó (mesh-only)
│   └── debug/              # painel de performance do host e do wm
└── private/                # interno/operador; não é exemplo público
    └── seance/             # instala e supervisiona o agente do Seance
```

O control plane semeia os manifestos em `<data_dir>/extensions/<id>/` no
primeiro boot (sem sobrescrever edições existentes). O layout de runtime é
**plano**: a pasta `public/`/`private/` organiza o repositório, não o data dir.
Scripts de hook embutidos são arquivos gerenciados e atualizados no boot; para
customizar, copie a extensão para outro id. O agente também semeia os assets
embutidos no data dir do nó, para que os hooks encontrem os scripts mesmo em nós
remotos. Extensões declarativas adicionais podem ser criadas diretamente no
data dir.

| id | visibilidade | driver | o que faz |
| --- | --- | --- | --- |
| `cloudflared` | pública | `cloudflared` | rotas de projeto para quick/named tunnel; ativável por nó |
| `chat` | pública | `exec` | sala de chat web em `127.0.0.1:8790`, share `chat-<nó>` e porta exposta |
| `debug` | pública | `exec` | painel de performance em `127.0.0.1:8791`, share `debug-<nó>` e porta exposta |
| `seance` | privada | `exec` | instala/roda o agente do Seance, expõe 8421 e publica `baguette-<nó>` |

A classificação é declarada no manifesto (`visibility = "public" | "private"`) e
aparece no painel. Ela é metadado de catálogo: não muda o loader nem o
supervisor. Por padrão `public`; extensões sem o campo continuam válidas.

Manifestos semeados antes desta classificação não são sobrescritos (o control
preserva edições do operador). Em um data dir antigo, edite ou remova
`<data_dir>/extensions/seance/extension.toml` e recarregue os manifestos para
que o Seance passe a aparecer como privado.

## Ativação

Extensões com componente no nó são **por nó** e começam desativadas. Ative na
aba **Extensões** da TUI (`wm tui`, tecla `Enter`), no painel web ou pela API:

```sh
wm extensions                # lista o catálogo deste nó
wm extensions enable chat    # ativa no nó atual
wm extensions disable chat
```

Ativar `seance` faz o agente rodar `install.sh` → `activate.sh` → sidecar
`seance-agent`, além de aplicar labels, share e expose declarados no adapter.

## Chat público

Ative o chat **no nó que hospeda a sala** (normalmente o servidor). Os outros
nós apenas abrem o share; não precisam ativar a extensão.

```sh
# no nó servidor
sudo wm extensions enable chat

# em outro nó
wm open chat-<nó-servidor>
```

O hook de ativação grava `/usr/local/bin/wm-chat` apontando para o binário `wm`
do nó. O sidecar sobe `wm chat --listen 127.0.0.1:8790`; o adapter então:

- cria o label `chat=true` e `chat_port=8790`;
- expõe `8790` no overlay (`10.66.0.x:8790` → `127.0.0.1:8790`, sujeito à ACL);
- publica o share web `chat-<nó>`, abrível com `wm open chat-<nó>` de outro nó.

O chat é **mesh-only**: o servidor escuta só em loopback e a única exposição é
a porta do adapter no overlay. Um nó com portas públicas fechadas continua
acessando o chat do nó servidor pelo túnel da malha; não é preciso abrir porta
no host nem no roteador. A sala **não tem autenticação**: qualquer nó que
alcance a porta pode ler e escrever; a ACL da malha é o controle de acesso. O
histórico fica em memória (até 500 mensagens) e reinicia junto com o sidecar; o
endereço some ao desativar a extensão. Não publique esta porta no cloudflared.

## Debug público

Ativar `debug` no nó:

```sh
sudo wm extensions enable debug
```

O hook grava `/usr/local/bin/wm-debug` apontando para o `wm` do nó. O sidecar
sobe `wm debug --listen 127.0.0.1:8791` e o adapter publica `debug-<nó>`,
expõe `8791` no overlay e aplica `debug=true`/`debug_port=8791`. O painel mostra
CPU, memória, load, discos, rede, o processo `wm` e os top processos. Também é
**mesh-only** e sem autenticação; trate os dados do host como sensíveis e
mantenha a porta fora da internet.

## Criar uma extensão

- **Pública:** crie `public/<id>/extension.toml`, use `visibility = "public"` e
  inclua `README.md` com o contrato e o modelo de segurança.
- **Privada:** crie `private/<id>/extension.toml` e use
  `visibility = "private"`. Não a documente como exemplo público.

O contrato completo está em [`docs/extensions.md`](../docs/extensions.md) e
[`docs/hooks-adapters.md`](../docs/hooks-adapters.md). O mapa carrega o comando
dos hooks, não os arquivos: para customizar scripts, instale-os no nó (ou use um
comando inline). Extensões embutidas são embutidas com `include_str!` em
`crates/wm-ext/src/manifest.rs`; ao mover ou criar uma pasta aqui, atualize os
caminhos e o seeder.

# Extensões do wired-magic

**Data:** 2026-11-12
**Escopo:** como o runtime de extensões funciona, como o cloudflared é integrado e
como criar novas extensões.
**Relacionado:** [ADR 0006](adr/0006-extension-runtime.md) ·
[ADR 0007](adr/0007-hooks-adapters.md) ·
[hooks-adapters.md](hooks-adapters.md) ·
[RESEARCH §3](RESEARCH.md#3-control-plane-e-mapa-por-long-poll) ·
[RESEARCH §7](RESEARCH.md#7-relay-wss)

---

## 1. O que é uma extensão

Uma extensão tem duas metades independentes:

| Metade | Onde roda | Responsabilidade |
| --- | --- | --- |
| **Control side** | control plane (`wm host`/`wm serve`) | manifesto, API admin, tabelas, estado desejado por nó |
| **Node side** | agente (`wm up`/`wm join`) | sidecar supervisionado, reconciliação e report do estado |

O caminho de dados é o que já existe — não há porta, token, socket ou conexão
nova para operar:

```text
   admin / painel
        │  HTTPS + cookie/CSRF
        ▼
  control plane ── desejo por nó ──► NetworkMap (long-poll de 25 s) ──► agente
        ▲                                                                  │
        │                    ReportRequest (30 s / mudança de estado)      │
        └──────────────────────────────────────────────────────────────────┘
                                                                           │
                                                              driver roda o sidecar
                                                              (cloudflared, exec, ...)
```

O `NetworkMap` carrega a lista **autoritativa** de extensões daquele nó:
uma extensão ausente da lista deve parar. O agente compara `revision`, `enabled`
e o JSON de `config`; se nada mudou, não reinicia o sidecar, mesmo quando a
versão do mapa muda por causa de peers ou ACL.

**Ativação é por nó.** Extensões node-side começam inativas em nós novos; a TUI
(`aba Extensões`, `Enter/t`), o painel web e a API alternam. Manifestos podem
declarar `default_config` para que ativar já materialize a config. Hooks e
adapters são detalhados em [hooks-adapters.md](hooks-adapters.md). A extensão
`chat_gui`/`debug_gui` são as receitas públicas de referência (sidecar +
adapter + UI web); `chat`/`debug` são as TUIs padrão embutidas no `wm` e
`seance` é a integração privada que monta um produto no nó.

O `ReportRequest` devolve o estado observado. O control guarda a última foto em
`extension_status`, o painel faz merge e o SSE invalida as consultas. O
`wm gui` do próprio nó também mostra as extensões ativas e a primeira URL
pública de cada uma.

### 1.1 Por que reaproveitar o mapa e o report

- **Zero transporte novo.** Já existe autenticação de nó, long-poll versionado,
  reconexão com backoff e tolerância a control fora.
- **Estado desejado é atômico.** O mapa inteiro é substituído; não há delta
  perdido nem ordem de patches.
- **Por nó.** O builder do mapa sabe quem é `me`; credenciais de cloudflared só
  vão para o nó dono.
- **Auditável.** O mesmo contrato JSON aparece no painel, nos testes e no
  agente.

Limite consciente: o mapa é O(nós + extensões) e o report é O(extensões). Para
centenas de nós e poucas extensões, isso é barato; se virar gargalo, o próximo
passo é um endpoint dedicado, não um delta dentro do mapa.

---

## 2. Manifesto

Toda extensão tem um diretório em:

```text
<data_dir>/extensions/<id>/extension.toml
```

O control plane semeia as extensões embutidas no primeiro boot. Um manifesto
inválido vira **aviso** e é ignorado; o cloudflared tem fallback embutido para
que o painel nunca fique sem a rota principal.

```toml
id = "hello"                 # slug 1-63, igual ao nome do diretório
name = "Hello Sidecar"
version = "0.1.0"
description = "Um sidecar declarativo de exemplo."
author = "operação"
homepage = "https://example.com/hello" # opcional
visibility = "public"        # public (default) | private
node_driver = "exec"         # none | exec | cloudflared
ui = "generic"               # cloudflared | generic (default)

[config_example]             # documentação; não aplica sozinho
command = "/usr/local/bin/hello"
args = ["--port", "9000"]
restart = "on_failure"
public_urls = ["https://hello.example.com"]
```

Regras:

- `id` é um slug `[a-z0-9_-]`, igual ao diretório; nomes fora disso são ignorados.
  O `_` permite variantes como `chat` (TUI) e `chat_gui` (web).
- `builtin` não é escrito pelo autor: o loader decide pelo id.
- `node_driver = "cloudflared"` é reservado ao id `cloudflared`.
- Campos desconhecidos no TOML são rejeitados para evitar knob silencioso.
- `ui = "cloudflared"` faz o painel React renderizar o gerenciador de rotas; os
  demais usam o editor genérico de JSON.
- `visibility = "public" | "private"` classifica a extensão no catálogo (padrão
  `public`). É metadado: o loader e o supervisor tratam as duas igualmente. A
  separação de pastas está em [`extensions/README.md`](../extensions/README.md).
- `default_active = false` mantém a extensão no catálogo, mas sem rodar até ser
  ativada no nó (cloudflared e seance usam o fallback de compatibilidade).
- `default_config`, `hooks` e `[adapter]` são opcionais; veja
  [hooks-adapters.md](hooks-adapters.md).
- O loader não é hot-reload de código: `POST /api/v1/admin/extensions/reload`
  relê manifestos e configurações, mas o binário do control não muda.

### 2.1 Adicionar uma extensão declarativa sem recompilar

1. Crie `<data_dir>/extensions/hello/extension.toml` como acima.
2. No painel, abra **Extensões → Hello Sidecar**.
3. Escolha o nó e salve a configuração JSON:

   ```json
   {
     "command": "/usr/local/bin/hello",
     "args": ["--port", "9000"],
     "env": {"HELLO_ENV": "prod"},
     "working_dir": "/opt/hello",
     "public_urls": ["https://hello.example.com"],
     "restart": "on_failure"
   }
   ```

4. Ative a extensão no nó (TUI/painel/API). O control incrementa a versão do
   mapa; o agente baixa a nova config, roda hooks e inicia o processo. O painel
   mostra `starting` → `running` (ou `error` com a última linha).
5. `restart` aceita `always`, `on_failure` (default) e `never`. O agente usa
   backoff de 1 s a 30 s e reseta o backoff após 60 s de vida do processo.

O driver `exec` roda o comando **diretamente, sem shell**, como o usuário do
agente (root, na prática). `args` é uma lista; `env` aceita nomes
`[A-Za-z_][A-Za-z0-9_]*`; `public_urls` é apenas informativo para o painel.

### 2.2 Extensões públicas e privadas

A pasta [`extensions/`](../extensions/) do repositório separa o catálogo aberto
do material interno:

```text
extensions/public/     # cloudflared, chat, chat_gui, debug, debug_gui
extensions/private/    # seance, debug-publish
```

O control continua semeando tudo em `<data_dir>/extensions/<id>/` (layout
plano). A classificação aparece no painel e vem do campo `visibility` do
manifesto. Extensões públicas devem trazer um `README.md` com contrato e modelo
de segurança; privadas não são documentadas como exemplo.

As extensões `chat_gui` e `debug_gui` mostram o ciclo completo de uma extensão
pública declarativa: sidecar `exec`, hook de ativação que prepara o wrapper,
adapter com label + share + expose e uma UI própria servida pelo sidecar. Ambas
são mesh-only: escutam em loopback e só são expostas no overlay, então um nó com
portas públicas fechadas continua acessando o serviço do nó servidor pelo
túnel da malha. As TUIs `wm chat` e `wm debug` (`chat`/`debug`,
`node_driver = "none"`) consomem a mesma API local ou o sampler local. Guias em
[`extensions/public/chat_gui/README.md`](../extensions/public/chat_gui/README.md),
[`extensions/public/debug_gui/README.md`](../extensions/public/debug_gui/README.md),
[`extensions/public/chat/README.md`](../extensions/public/chat/README.md) e
[`extensions/public/debug/README.md`](../extensions/public/debug/README.md).

### 2.3 Pedidos do servidor

O admin pode pedir que um nó rode uma extensão:

```http
PUT /api/v1/admin/extensions/chat_gui/nodes/{node_id}/request
{ "requested": true, "message": "rode o chat_gui neste nó" }
```

A solicitação é uma **recomendação**: aparece no catálogo do nó
(`NodeExtensionView.requested`/`request_message`) com `!` na TUI, mas não ativa
nem executa nada. `requested: false` remove. A ativação continua sendo uma
decisão do membro.

### 2.4 Gerenciador de pacotes (git, estilo flake)

Um repositório git é uma fonte de pacotes. Qualquer subdiretório com
`extension.toml` é um pacote; opcionalmente, `wm-flake.toml` na raiz lista os
pacotes:

```toml
[flake]
name = "acme-extensions"
version = "1"

[[package]]
id = "chat_gui"
path = "packages/chat_gui"
```

```sh
wm packages discover github:owner/repo
wm packages install  github:owner/repo --package chat_gui --yes
wm packages install  github:owner/repo --ref v1.2.0 --subdir packages --all --yes
wm packages list
wm packages update
wm packages remove chat_gui --yes
```

O clone vai para `<data_dir>/extensions/.sources/<nome>`, o pacote é copiado
para `<data_dir>/extensions/<id>/` e o lock
`<data_dir>/extensions/.packages.json` registra URL, ref, commit e subdir.
`--yes` confirma a execução de código; `--dry-run` só mostra. O gerenciador não
resolve `inputs`/dependências de outros flakes automaticamente e não avalia
Nix: o “flake” aqui é o contrato repo + ref + subdir + `extension.toml`.

### 2.5 TUI fornecida pela extensão

Uma extensão pode declarar uma **aba própria** na TUI principal. O manifesto
ganha uma seção `[tui]`:

```toml
[tui]
tab = "Chat"
mode = "pane"          # pane | snapshot | exec
command = "{wm}"
args = ["chat", "--url", "{option:url}"]
refresh_secs = 5       # usado no modo snapshot
options = { url = "http://127.0.0.1:8790" }
```

- `mode = "pane"`: a extensão roda dentro de um **PTY embutido** no corpo da
  TUI principal. O wm fornece o terminal, o redimensionamento e encaminha as
  teclas; a extensão desenha a interface que quiser (ratatui, curses, shell).
  `Tab`/`Shift+Tab` troca de aba; as demais teclas vão para a extensão. Se o
  processo sair, `Enter` reinicia a aba.
- `mode = "exec"`: a TUI principal é suspensa e o comando roda em tela cheia;
  ao sair, a TUI é restaurada na mesma aba.
- `mode = "snapshot"`: a TUI executa o comando periodicamente e renderiza o
  JSON de `TuiDocument` abaixo. O comando deve terminar rápido e imprimir
  somente o JSON em stdout.
- `{wm}` é substituído pelo binário atual; `{extension_dir}` pelo diretório
  local da extensão; `{option:chave}` por uma opção de `[tui.options]`.
- Variáveis no comando: `WM_EXTENSION_ID`, `WM_DATA_DIR`, `WM_TUI=1` (e
  `WM_TUI_PANE=1` no modo pane).
- Até 8 abas de extensão são exibidas; o catálogo local inclui os manifestos
  embutidos mesmo antes do data dir ser semeado.

Contrato JSON do `mode = "snapshot"` (definido em `wm-proto::TuiDocument`):

```json
{
  "title": "Minha extensão",
  "subtitle": "resumo opcional",
  "sections": [
    {
      "title": "Status",
      "lines": ["linha 1", "linha 2"]
    },
    {
      "title": "Tabela",
      "columns": ["Nome", "Valor"],
      "rows": [["cpu", "12%"], ["ram", "1.2 GB"]]
    }
  ],
  "footer": "rodapé opcional"
}
```

O comando roda com o privilégio do usuário da TUI (root, na prática). Trate o
manifesto como código confiável, como hooks `exec`.

Exemplo completo em
[`examples/extensions/tui-panel`](../examples/extensions/tui-panel).

> **Modelo de confiança:** só um admin do control pode gravar configuração, e a
> extensão roda com o privilégio do agente. Trate o JSON de `exec` como uma unit
> de systemd: não aponte para binários que você não confiaria em root.

---

## 3. Cloudflared (extensão embutida)

O cloudflared expõe um serviço **loopback do próprio nó** pela Cloudflare. Ele
não dá ao nó um IP público nem abre portas no roteador; o sidecar faz conexões
de saída para a borda da Cloudflare.

### 3.1 Os dois modos

| Modo | Processo por nó | URL pública | Conta Cloudflare | Uso |
| --- | --- | --- | --- | --- |
| `quick` | um por rota | `https://<aleatório>.trycloudflare.com` (efêmera) | não | demo, teste, acesso rápido |
| `named` | um para todas as rotas named | hostnames do seu DNS | sim (túnel + credenciais) | projeto real, Cloudflare Access |

Os dois modos coexistem no mesmo nó. O agente roda:

```text
# quick, uma instância por rota
cloudflared tunnel --no-autoupdate --url http://127.0.0.1:3000

# named, uma instância por nó
cloudflared tunnel --no-autoupdate --config <data_dir>/extensions/cloudflared/config.yml run
```

O config do named é renderizado pelo agente com um `ingress` por hostname e um
catch-all `http_status:404`. O credentials JSON é gravado com permissão `0600`
em `<data_dir>/extensions/cloudflared/credentials.json`.

### 3.2 Pré-requisitos no nó

- `cloudflared` no `PATH`, ou o caminho absoluto configurado em **Nós e túneis →
  Configurar**.
- Para `named`: um túnel criado na conta Cloudflare, o UUID e o JSON de
  credenciais; DNS do hostname roteado para o túnel.

Criação do túnel named (na máquina do operador, uma vez):

```sh
cloudflared tunnel login
cloudflared tunnel create seance
# anote o UUID e o arquivo ~/.cloudflared/<uuid>.json
cloudflared tunnel route dns seance app.exemplo.com
```

Cole o **conteúdo do JSON** no painel, não o token do túnel. O agente não roda
`tunnel route dns`; ele só sobe o túnel com o ingress local.

### 3.3 Fluxo no painel

1. **Extensões → Cloudflare Tunnel**.
2. Em **Nós e túneis**, escolha o nó e clique **Configurar**:
   - binário (`cloudflared` ou caminho);
   - tunnel ID (UUID);
   - credentials JSON; marque **Substituir** para trocar um segredo existente.
3. **Nova rota**:
   - projeto: slug curto (`seance-site`);
   - nó;
   - serviço local: `http://127.0.0.1:3000` (somente loopback);
   - modo quick ou named + hostname;
   - ativa.
4. Acompanhe **Público / status**. No quick, a URL aparece em alguns segundos.
5. **Reiniciar** incrementa a revisão do nó e força o agente a recriar os
   processos, sem esperar uma mudança de config.

### 3.4 API

Todas as rotas exigem sessão admin + `X-WM-CSRF` nas mutações.

| Método | Rota | Efeito |
| --- | --- | --- |
| GET | `/api/v1/admin/extensions/cloudflared/overview` | `{extension, nodes, routes}` em uma chamada |
| GET | `/api/v1/admin/extensions/cloudflared/routes` | rotas + status vivo |
| POST | `/api/v1/admin/extensions/cloudflared/routes` | cria rota |
| PUT | `/api/v1/admin/extensions/cloudflared/routes/{id}` | substitui rota |
| DELETE | `/api/v1/admin/extensions/cloudflared/routes/{id}` | remove rota |
| GET | `/api/v1/admin/extensions/cloudflared/nodes` | configuração/estado por nó |
| PUT | `/api/v1/admin/extensions/cloudflared/nodes/{node_id}` | binário/túnel/credenciais |
| POST | `/api/v1/admin/extensions/cloudflared/nodes/{node_id}/restart` | força reconciliação |

Corpo de rota:

```json
{
  "project": "seance-site",
  "node_id": "…",
  "local_url": "http://127.0.0.1:3000",
  "mode": "quick",
  "hostname": null,
  "enabled": true
}
```

Validações que o control aplica antes de gravar:

- `local_url` precisa ser `http(s)://` com porta e host loopback (`127.0.0.1`,
  `localhost`, `::1`); query, fragmento e userinfo são recusados;
- `project` é slug de 1-63;
- `hostname` named é FQDN válido, único entre as rotas;
- rota named exige tunnel + credenciais configurados no nó;
- máximo de 64 rotas por nó;
- nó desativado não recebe rota.

O agente reporta o estado no formato:

```json
{
  "id": "cloudflared",
  "state": "running",
  "public_urls": ["https://random.trycloudflare.com"],
  "details": {
    "routes": [
      {"id": "…", "project": "seance-site", "state": "running",
       "public_url": "https://random.trycloudflare.com"}
    ]
  }
}
```

### 3.5 Segurança e limites

- **Quick é público.** Qualquer pessoa com a URL alcança o serviço; não há
  autenticação. Use apenas para demonstração. Para produção, named + Cloudflare
  Access (ou uma camada de auth na aplicação).
- **Somente loopback.** O agente recusa alvos como `10.66.0.3:8421`; um nó não
  vira pivô para expor o serviço de outro nó. Se o projeto estiver em outro
  host, exponha-o localmente primeiro (por exemplo, com o `--expose` do agente
  ou um proxy local).
- **Credenciais.** Ficam no SQLite do control (`data_dir`, arquivo `0600`) e só
  são incluídas no mapa daquele nó. Não há endpoint que devolva o segredo; o
  painel mostra apenas `has_credentials`. O control não tem cofre/KMS no v1;
  proteja o backup do banco.
- **Revogação.** Desabilitar o nó revoga o token; o agente recebe `401` no mapa
  e o supervisor para os sidecars. Um control fora do ar **não** para sidecars
  (tolerância a queda).
- **Host header.** O cloudflared preserva o Host original para o serviço local;
  se o app valida Host, ajuste a allowlist do app ou use `originRequest`.
- **Um processo named por nó.** Uma credencial por nó no v1; múltiplos túneis
  named no mesmo nó exigiriam agrupar por tunnel, o que não está exposto no
  painel.

### 3.6 Diagnóstico

| Sintoma | Causa provável | Ação |
| --- | --- | --- |
| `error: falha ao executar 'cloudflared'` | binário ausente no nó | instale ou configure o caminho |
| `TunnelID das credenciais não confere` | JSON de outro túnel | confira o UUID e o `TunnelID` |
| `rotas named exigem o túnel do nó` | settings ausentes | preencha tunnel ID + credentials |
| `hostname_taken` | duas rotas com o mesmo FQDN | renomeie ou remova a outra |
| quick fica `starting` | rede bloqueando a borda | veja a mensagem do último log |
| rota named `error` | credencial inválida/DNS ausente | valide no dashboard/`cloudflared` |
| `401` no report | nó revogado/desabilitado | reative o nó; sidecars param por segurança |

---

## 4. Criar uma extensão code-backed

Quando `exec` não basta (precisa de API própria, tabelas, UI, múltiplos
processos ou parsing de log), implemente um driver.

### 4.1 Contratos compartilhados (`crates/wm-ext`)

- `ExtensionManifest`, `NodeDriver`, loader e validação;
- `ExecConfig` e `CloudflaredConfig` com validação;
- helpers de URL/hostname/credenciais e parser de URL quick.

Novos contratos de configuração devem morar ali (ou em `wm-proto`, se forem
tipos de fio) para serem testáveis sem runtime.

### 4.2 Control side (`wm-control`)

1. Adicione as tabelas no `init_schema` (`db.rs`) ou use `extension_records`.
2. Implemente `ControlExtension` em `extensions/<id>.rs`:

   ```rust
   #[async_trait]
   impl ControlExtension for MinhaExt {
       fn id(&self) -> &'static str { "minha-ext" }
       fn manifest(&self) -> ExtensionManifest { wm_ext::... }
       fn router(&self) -> Router<Arc<AppState>> { /* rotas admin */ }
       async fn desired_for(&self, state: &AppState, node: &NodeRow)
           -> anyhow::Result<Option<ExtensionDesired>> { /* ... */ }
   }
   ```

3. Registre em `ExtensionRegistry::load` (mapa `code`).
4. Valide entradas na API; faça `state.bump_version()` em toda mutação que afeta
   o mapa; registre `db::audit`.
5. Se precisar de config genérica por nó, use `extension_records`; se for
   específica, mantenha tabelas próprias e documente.

### 4.3 Node side (`wm-core`)

1. Crie `extensions/<driver>.rs` com uma função `run(config, reporter, cancel)`.
2. Use `process::ManagedProcess` para spawn/linhas/kill; ele garante
   `kill_on_drop`.
3. Atualize o `Reporter` com `state`, `message`, `public_urls` e `details`.
4. Adicione o `match` em `extensions::spawn_driver`.
5. Respeite `CancellationToken`; o supervisor aborta a tarefa no shutdown ou na
   remoção da lista.
6. Teste com um fake script (veja os testes de `cloudflared` quick) para não
   depender do binário real.

### 4.4 API genérica

Mesmo sem código, toda extensão ganha:

| Método | Rota |
| --- | --- |
| GET | `/api/v1/admin/extensions` |
| GET | `/api/v1/admin/extensions/{id}` |
| POST | `/api/v1/admin/extensions/reload` |
| GET | `/api/v1/admin/extensions/{id}/nodes/{node_id}/config` |
| PUT | `/api/v1/admin/extensions/{id}/nodes/{node_id}/config` |
| DELETE | `/api/v1/admin/extensions/{id}/nodes/{node_id}/config` |
| PUT | `/api/v1/admin/extensions/{id}/nodes/{node_id}/activation` |
| PUT | `/api/v1/admin/extensions/{id}/nodes/{node_id}/request` |
| GET | `/api/v1/node/extensions` |
| PUT | `/api/v1/node/extensions/{id}` |

Extensões code-backed podem recusar a config genérica e expor a própria API
(como o cloudflared).

---

## 5. Modelo de dados

```sql
extension_records   (extension_id, node_id, enabled, config, revision, updated_at, updated_by)
extension_requests  (extension_id, node_id, message, updated_at, updated_by)
extension_status    (extension_id, node_id, name, driver, state, message, public_urls, details, updated_at)
cloudflared_routes (id, project, node_id, local_url, mode, hostname, enabled, created_at, updated_at)
cloudflared_nodes  (node_id, binary, tunnel_id, credentials, revision, updated_at)
```

- `revision` é monotônica por (extensão, nó). Mudanças de config e o botão
  **Reiniciar** incrementam; o agente reinicia quando a revisão muda.
- `extension_status` é substituída a cada report; status de extensões que
  sumiram da lista são removidos.
- Segredos (credentials) não aparecem em views nem em logs; a entrega é só no
  mapa do nó dono.

---

## 6. Testes

```sh
cargo test -p wm-ext            # manifesto, validação, YAML, parser de URL
cargo test -p wm-control        # store, API, mapa + report ponta a ponta
cargo test -p wm-core           # supervisor, exec fake, quick fake
cargo test --workspace
```

O teste `crates/wm-control/tests/extensions_api.rs` sobe o router axum, faz
login admin, cria nó, configura túnel, cria rotas, lê o mapa como o nó e envia
um report; depois confere o status no painel. `wm-core` sobe um script fake que
imprime uma URL `trycloudflare.com` e valida o parsing e a reconciliação.

---

## 7. Limites e próximos passos

- Sem sandbox: `exec` roda como root; o modelo é “admin confiável”.
- Sem assinatura de manifesto nem de driver; o diretório de extensões é
  root-owned.
- Sem hot-reload de código; recarregar manifestos é só metadata.
- Sem UI custom por extensão declarativa; o editor genérico cobre JSON.
- Sem múltiplos túneis named por nó.
- Sem integração automática entre uma rota cloudflared e um share `wm://`; o
  campo `public_url` de share existe e é o ponto de extensão.
- Sem logs históricos no control; o status carrega a última mensagem. Para logs
  completos, use `journalctl`/Console no nó.
- Remover o manifesto/extensão para o sidecar, mas não apaga credenciais ou
  configs já gravadas em `<data_dir>/extensions/cloudflared/` no nó; limpe esse
  diretório manualmente se o segredo sair de uso.


---

## 8. Ativação, hooks e adapters

Desde o ADR 0007, `extension.toml` pode declarar:

```toml
default_active = false

[default_config]
command = "/usr/local/bin/meu-agente"
args = ["--port", "9000"]

[hooks.install]
command = "{extension_dir}/install.sh"
timeout_secs = 300

[hooks.activate]
command = "{extension_dir}/activate.sh"

[hooks.deactivate]
command = "{extension_dir}/deactivate.sh"

[adapter]
labels = { produto = "meu-produto" }
exposes = [{ overlay_port = 9000, local_port = 9000 }]

[[adapter.shares]]
name = "meu-produto-{node}"
port = 9000
kind = "web"
protocol = "tcp"
```

O ciclo completo — quando cada hook roda, variáveis `WM_*`, placeholders,
propriedade de shares, exposes dinâmicos e a receita do Seance — está em
[hooks-adapters.md](hooks-adapters.md). A pasta [`extensions/`](../extensions/)
no repositório é a fonte de verdade dos bundlados, dividida em `public/` e
`private/`; o control semeia `<data_dir>/extensions/` no boot sem sobrescrever
manifestos editados (scripts embutidos de hook são arquivos gerenciados e
atualizados).

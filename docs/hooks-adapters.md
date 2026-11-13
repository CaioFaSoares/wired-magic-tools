# Hooks e adapters

**Data:** 2026-11-13
**Escopo:** como uma extensão reage ao ciclo de vida no nó (hooks) e como
declara serviços/labels/exposes no control plane (adapter).
**Relacionado:** [extensions.md](extensions.md), [ADR 0007](adr/0007-hooks-adapters.md),
[SHARES.md](SHARES.md)

---

## 1. Modelo

```text
manifesto (extensions/<id>/extension.toml)
   │
   ├── hooks        comandos no NÓ, em install/activate/deactivate
   ├── adapter      efeitos no CONTROL: labels + shares + exposes
   └── driver       processo supervisionado no nó (exec/cloudflared)
```

- **Hook** é uma ação de lifecycle no nó. Roda uma vez (install) ou a cada
  entrada/saída de ativação (activate/deactivate).
- **Adapter** é uma tradução declarativa de um produto para primitivas da malha:
  uma label no nó, um share no mapa e uma porta exposta no overlay.
- O **driver** continua sendo o processo de longa duração.

Ativação é por nó. O catálogo vem do control (`GET /api/v1/node/extensions`) e a
TUI usa `Enter/t` para alternar. Ativar pode materializar um `default_config` e
disparar install/activate; desativar para o sidecar e roda deactivate.

---

## 2. Hooks

```toml
[hooks.install]
command = "{extension_dir}/install.sh"
timeout_secs = 300
working_dir = "{extension_dir}"          # opcional
[hooks.install.env]
SEANCE_AGENT_URL = "https://exemplo/seance-agent"

[hooks.activate]
command = "{extension_dir}/activate.sh"
timeout_secs = 60

[hooks.deactivate]
command = "{extension_dir}/deactivate.sh"
timeout_secs = 60
```

### 2.1 Quando rodam

| hook | quando | garantia |
| --- | --- | --- |
| `install` | na primeira ativação da versão do manifesto | marcador em `<data_dir>/extensions/<id>/hooks/install-<versão>.ok` |
| `activate` | antes do driver, em cada start/reconciliação da extensão | timeout e resultado no status |
| `deactivate` | depois que o driver para, na desativação/remoção | best-effort; erro só no log |

Uma mudança de config reinicia o driver e, portanto, roda deactivate → activate
de novo. Isso é intencional: o hook recebe a config nova em
`WM_EXTENSION_CONFIG`. O `install` não repete enquanto a versão não muda.

Os hooks são serializados por extensão: o activate novo espera o deactivate
antigo terminar. Um timeout no deactivate não bloqueia o próximo start por mais
que o limite configurado.

### 2.2 Ambiente

| variável | valor |
| --- | --- |
| `WM_NODE_ID`, `WM_NODE_NAME` | identidade do nó |
| `WM_OVERLAY_IP` | IP do nó na malha |
| `WM_CONTROL_URL` | URL pública do control |
| `WM_DATA_DIR` | data dir do agente |
| `WM_EXTENSION_ID`, `WM_EXTENSION_NAME`, `WM_EXTENSION_VERSION`, `WM_EXTENSION_DRIVER` | extensão |
| `WM_EXTENSION_CONFIG` | JSON da config do nó |
| `WM_EXTENSION_CONFIG_FILE` | caminho do `config.json` (0600) |
| `WM_EXTENSION_DIR` | diretório da extensão no nó |
| `WM_HOOK` | `install`, `activate` ou `deactivate` |

Placeholders equivalentes em `command`, `args`, `env` e `working_dir`:
`{node_id}`, `{node_name}`, `{overlay_ip}`, `{control_url}`, `{data_dir}`,
`{extension_id}`, `{extension_version}`, `{extension_dir}`, `{hook}`.

O comando roda **sem shell**; para usar shell, aponte explicitamente para
`/bin/sh` com `args = ["-c", "..."]`. Hooks rodam com o privilégio do agente
(root). O manifesto é conteúdo de admin confiável — trate como uma unit.

### 2.3 Falha

- `install`/`activate` com exit != 0: status `error`, driver **não** inicia, a
  última linha da saída vira a mensagem.
- timeout: processo é morto (kill_on_drop) e status `error`.
- `deactivate`: erro/tempo esgotado só aparece no log do agente; a remoção da
  extensão prossegue.

---

## 3. Adapters

```toml
[adapter]
labels = { seance = "true", baguette_port = "8421" }
exposes = [{ overlay_port = 8421, local_port = 8421 }]

[[adapter.shares]]
name = "baguette-{node}"
port = 8421
kind = "web"
protocol = "tcp"
description = "Baguette do Seance"
```

`{node}`/`{extension}` nos nomes de share são normalizados para
`[a-z0-9-]` (nomes de nó podem ter `.`/`_`). Enquanto a extensão está **ativa
naquele nó**:

- `labels` são mescladas nas labels do nó;
- cada share é criado/atualizado no control, dono = nó;
- `exposes` entram em `NetworkMap.exposes` e o agente reconcilia listeners
  `overlay:porta → 127.0.0.1:porta` sem reiniciar.

Ao desativar:

- labels são removidas **somente se o valor ainda for o do adapter** (uma
  mudança manual do operador não é sobrescrita);
- shares de propriedade do adapter são removidos (controle em
  `extension_adapter_resources`; um share homônimo do usuário não é tomado);
- exposes saem do mapa e os listeners correspondentes são fechados.

O adapter é idempotente e roda na ativação, no boot do control e a cada 30 s,
para recriar um share apagado à mão ou corrigir uma label. Exposes são
projetados no mapa a cada build; não há estado extra.

### 3.1 Exposes dinâmicos

`NetworkMap.exposes` é a lista de portas que o nó deve publicar no overlay.
O agente faz a união com `--expose`/estado local (a base vence em caso de
conflito), abre um `TcpListener` no IP do overlay e faz `copy_bidirectional`
para `127.0.0.1:<porta local>`, sujeito à ACL do data plane. Nó revogado/pendente
limpa os exposes de extensão.

---

## 4. Seance como referência

`extensions/seance/` demonstra o fluxo completo:

1. `default_config` aponta para `/usr/local/bin/seance-agent`; o `install.sh` copia para lá se o binário estiver em outro ponto do PATH.
2. `hooks.install` roda `install.sh` uma vez; o script baixa
   `SEANCE_AGENT_URL` (configurável em `[hooks.install.env]`) ou aceita o
   binário já no PATH.
3. Ativar na TUI (`wm tui` → Extensões → Seance) materializa a config, roda
   install/activate e inicia o sidecar.
4. O adapter publica `baguette-<nó>`, expõe `8421` e marca o nó com
   `seance=true`/`baguette_port=8421`.
5. O Seance descobre o nó pelo mapa/shares ou pela API de integração e conecta
   em `http://10.66.0.x:8421/`.

```sh
# no nó, depois de instalar o binário do wm:
wm extensions                 # catálogo
wm extensions enable seance  # instala, ativa, expõe e publica
wm extensions disable seance # para o sidecar e desfaz adapter
```

O `install.sh` embutido é um template: ajuste `SEANCE_AGENT_URL` (ou troque o
comando) para o artefato real do Seance. O agente do Seance deve escutar o
Baguette em 127.0.0.1:8421 ou a porta do `default_config`/config do nó.

---

## 5. Criar um adapter

1. Crie `extensions/<id>/extension.toml`.
2. Defina `default_config` se ativar deve já iniciar o sidecar.
3. Defina `hooks.install/activate/deactivate` (opcional).
4. Defina `[adapter]` com labels/shares/exposes.
5. Recarregue os manifestos no painel (`POST /admin/extensions/reload`) ou
   reinicie o control (a pasta é semeada no boot).
6. Ative por nó na TUI/painel.

Não é necessário recompilar para adapters declarativos. Para efeitos que não
cabem em labels/share/expose (chamar a API do Seance, provisionar credenciais,
etc.), implemente um `ControlExtension` e um driver próprio; veja
[extensions.md](extensions.md).

---

## 6. Limites

- Hooks não são sandbox; rodam como root e podem fazer qualquer coisa no nó.
- Os scripts das extensões **embutidas** (`cloudflared`, `seance`) são semeados
  no data dir do agente e **atualizados no boot** (arquivos gerenciados; editar
  `seance/*.sh` no lugar não persiste). Para customizar, copie a extensão para
  outro id ou use um comando inline. Para uma extensão customizada, o arquivo apontado pelo
  hook precisa já existir no nó (o control não envia assets pelo mapa); instale-o
  por pacote/config ou aponte o hook para um comando já presente. Editar
  `control/extensions/<id>/*.sh` no servidor **não** envia o arquivo para nós
  remotos: o mapa carrega apenas o comando/args/env. Para um comando inline, use
  `command = "/bin/sh"` com `args = ["-c", "..."]`.
- Não há retry automático de install/activate; a mensagem fica no status e um
  novo start (restart/reativar) tenta de novo.
- Adapter shares são de propriedade do adapter; não edite o share esperando que
  o adapter preserve a edição (o próximo sync corrige).
- Exposes são TCP only no v1.
- Múltiplos adapters que declarem a mesma porta de overlay: o primeiro em
  ordem de id no mapa vence; o agente também deduplica.
- O control não aplica adapter em nó desativado (o mapa não é servido).

# Deploy com Coolify (Docker Compose)

Este é o caminho de menor atrito para colocar o wired-magic numa VPS Linux: **um recurso
Docker Compose** no Coolify sobe o control plane, o relay, o STUN e o nó gateway `10.66.0.1`.
O Coolify cuida do TLS/domínio do painel; o compose publica apenas as portas UDP que o proxy
HTTP não consegue transportar.

> **O que este caminho não faz:** o nó gateway roda dentro do container, na rede bridge do
> Docker. Ele participa da malha e os outros nós conseguem alcançá-lo pelo overlay, mas a
> **VPS em si** (processos no host, containers vizinhos) não enxerga `10.66.0.0/24`. Se você
> precisa que a própria VPS seja um nó de verdade — para alcançar as máquinas ou rotear a LAN
> dela — veja [A VPS como nó de verdade](#7-a-vps-como-nó-de-verdade-opcional).

## Topologia

```text
Internet
  │  443/tcp  ── Coolify/Traefik ──► container wm:8080 (painel, API, relay WSS)
  │  3478/udp ─────────────────────► container wm:3478 (STUN)
  │  41641/udp ────────────────────► container wm:41641 (WireGuard do nó gateway)
  ▼
VPS Linux
  └── container `wm` (wm host)
        ├── control plane + SQLite em /data/control
        ├── relay WSS em /relay
        ├── STUN em 3478/udp
        └── nó gateway 10.66.0.1 (TUN wm0 dentro do container)
              ▲
              │ overlay 10.66.0.0/24
        Macs / Linux com `wm join`
```

A topologia acima mostra as portas padrão. Configure `WM_HTTP_PORT` (8080),
`WM_STUN_PORT` (3478) e `WM_UDP_PORT` (41641) para usar outras portas. HTTP fica
somente na rede interna; o proxy do Coolify deve usar a porta `WM_HTTP_PORT` configurada.
As duas portas UDP são iguais dentro e fora do container: não altere só a porta externa,
pois isso pode fazer o serviço anunciar um endpoint incorreto.

O container precisa de `/dev/net/tun` e da capability `NET_ADMIN` para criar a interface TUN
e instalar rotas **dentro do próprio container**. Ele **não** roda `privileged` e não altera
rotas do host.

## Pré-requisitos

- VPS Linux (x86_64 ou aarch64) com Docker rootful e Coolify instalado.
- Domínio apontando para a VPS (A/AAAA) e portas liberadas. Se usar Cloudflare ou outra CDN,
  deixe o registro em modo **DNS only** (ou garanta que UDP passe): o STUN usa `3478/udp` no
  mesmo host e não atravessa um proxy HTTP.

| Porta | Protocolo | Uso |
| --- | --- | --- |
| 80 | TCP | ACME/redirect do Coolify (quando aplicável) |
| 443 | TCP | painel, API e relay WSS (pelo Traefik) |
| 3478 | UDP | STUN |
| 41641 | UDP | endpoint WireGuard do nó gateway (recomendado; sem ele o nó cai para relay) |

- `/dev/net/tun` no host:

  ```sh
  ls -l /dev/net/tun
  # se não existir:
  sudo modprobe tun
  ```

- Nenhum outro processo usando as portas UDP escolhidas (`WM_STUN_PORT` e `WM_UDP_PORT`,
  padrões `3478/udp` e `41641/udp`). Libere essas portas no firewall se alterar os padrões.
- O build Rust acontece no servidor do Coolify, com um job por padrão. Não inclui Node/React.
  Consumo e duração dependem do builder/cache; não há garantia de CPU/RAM sem medição.

## 1. Variáveis de ambiente

No Coolify, crie o recurso e cadastre em **Environment Variables**:

| Variável | Obrigatória | Exemplo | Observação |
| --- | --- | --- | --- |
| `WM_PUBLIC_URL` | sim | `https://mesh.exemplo.com` | URL pública **sem** `:8080` e sem barra no final; entra no painel, nos convites e nos tokens de relay |
| `WM_BOOTSTRAP_USERNAME` | volume novo | `setup` | conta temporária limitada ao setup pela TUI |
| `WM_BOOTSTRAP_PASSWORD` | volume novo | senha longa e única | bootstrap invalidado ao criar admin + usuário; remova depois |
| `WM_NODE_NAME` | não | `gateway` | nome do nó `.1` no painel; padrão `gateway` |
| `WM_HTTP_PORT` | não | `8080` | porta HTTP interna; configure a mesma porta no proxy do Coolify |
| `WM_STUN_PORT` | não | `3478` | porta STUN UDP, interna e externa iguais |
| `WM_UDP_PORT` | não | `41641` | porta WireGuard UDP do gateway, interna e externa iguais |
| `WM_LOG` | não | `info` | `trace`, `debug`, `info`, `warn` ou `error` |

Gere a senha com algo como:

```sh
openssl rand -base64 32
```

> **Não confunda as duas URLs.** O domínio do Coolify aponta para o serviço `wm` na porta
> interna **`WM_HTTP_PORT`** (padrão 8080; `https://mesh.exemplo.com:8080` no campo Domains).
> Já `WM_PUBLIC_URL` é a URL pública sem a porta interna (`https://mesh.exemplo.com`).
> Não copie a porta interna para essa URL: convites e relay apontariam para a porta errada.

O compose passa `--listen 0.0.0.0:<WM_HTTP_PORT>`,
`--stun-listen 0.0.0.0:<WM_STUN_PORT>` e `--udp-port <WM_UDP_PORT>` ao `wm host`.
`--stun-listen` recebe um `SocketAddr` (IP:porta). Para anunciar servidores STUN diferentes,
adicione `--stun-url stun:host:porta` ao `command`, repetindo a opção para cada servidor.
O endereço anunciado deve ser alcançável pelos clientes, nunca `0.0.0.0`.

## 2. Criar o recurso no Coolify

1. Abra o projeto/ambiente e clique em **+ New**.
2. Escolha o repositório (público ou deploy key) e a branch (`main`).
3. Em **Build pipeline**, escolha **Docker Compose**.
4. Preencha:
   - **Base Directory:** `/`
   - **Docker Compose Location:** `/docker-compose.yml`
5. Salve e abra **Domains**:
   - **Service:** `wm`
   - **Domain:** `https://mesh.exemplo.com:8080` (substitua `8080` pelo valor de `WM_HTTP_PORT`)
6. Cadastre as variáveis da seção anterior.
7. Clique em **Deploy**.

O Coolify constrói a imagem Rust headless e gera as labels do Traefik para o domínio.
Não há build Web obrigatório; o painel é um pacote opcional ativado posteriormente.

### Cache e builds locais

É necessário **BuildKit** (Docker Engine/Compose atuais, também no builder usado pelo Coolify).
O compose usa somente `build.context`, `dockerfile` e `target`; não requer Bake, cache remoto,
modo privilegiado de build nem uma plataforma fixa. A imagem é construída na arquitetura do
builder (ARM64 ou x86_64), sem configurar cross-compilation.

```sh
# As variáveis obrigatórias do compose continuam necessárias, mesmo para config/build.
docker compose config --quiet
docker compose build wm
docker compose up -d --no-build
```

- Mesmo commit: reutiliza as camadas completas.
- Mudança no frontend: não altera a imagem headless; gere e distribua o pacote opcional
  separadamente (veja [painel opcional](control-panel.md)).
- Mudança em Rust: caches de registry/git evitam baixar novamente; o cache de `target`
  reutiliza dependências compiladas, separado por arquitetura e versão de Rust.
- Não há `cargo clean` forçado nem assets React embutidos em `wm-control`. O binário é
  copiado para fora do cache mount; nenhum cache de build é incluído no runtime.
- Os caches pertencem ao **builder**, não ao volume `wm-data`: trocar o builder/servidor,
  garbage collection ou limpeza de cache no Coolify pode exigir downloads/build completos.
  `--no-cache` refaz camadas, mas **não esvazia** os cache mounts. Para uma medição realmente
  fria use um builder novo; não apague caches compartilhados de produção sem planejamento.

Não configure `DOCKER_BUILDKIT=0`. Em versões antigas do Coolify/Docker confirme suporte a
`RUN --mount=type=cache`; o deploy real no Coolify ainda precisa ser validado no seu servidor.
O `.dockerignore` exclui builds locais, dependências, estado e arquivos `.env*` (exceto o
exemplo), evitando contextos enormes e envio acidental de segredos. Variáveis de runtime
continuam sendo fornecidas pelo Compose/Coolify, não copiadas para a imagem.

O HTTP/API sobe primeiro. Após concluir o setup pela TUI, o gateway se inscreve
(`WM_NODE_NAME`, padrão `gateway`) e reivindica `10.66.0.1`. Não se exige token manual
para o gateway; convites para outras máquinas só são emitidos após o setup.

Se o repositório for privado, configure a deploy key no Coolify antes do deploy. O compose
usa `build.context: .`; não é necessário publicar imagem em registry.

## 3. Primeiro start e login

Acompanhe **Deployment Logs** e depois **Runtime Logs**. O healthcheck consulta
`http://127.0.0.1:<WM_HTTP_PORT>/healthz` dentro do container; quando ficar saudável:

```sh
curl -fsS https://mesh.exemplo.com/healthz
curl -fsS https://mesh.exemplo.com/api/v1/meta
```

> O primeiro start inscreve o gateway diretamente no listener HTTP local; o agente do host
> também usa esse caminho, sem depender do Traefik ou do healthcheck público. `WM_PUBLIC_URL`
> continua sendo a URL anunciada para painel, convites e clientes externos. Se você ativar TLS
> nativo no próprio `wm host`, ele mantém a URL pública para validar o certificado.
> Em versões antigas, `API 503: no available server` no cadastro podia impedir o startup:
> atualize e reconstrua a imagem; não é necessário apagar o volume.

A raiz HTTP é headless, não uma tela React. No seu computador, abra:

```sh
wm admin --server https://mesh.exemplo.com tui
```

Entre com `WM_BOOTSTRAP_USERNAME` / `WM_BOOTSTRAP_PASSWORD`; crie a conta admin
permanente e a conta de usuário cotidiano. Confirme com `y`. O bootstrap é
invalidado definitivamente; a TUI entra como usuário. Saia e entre como admin
quando precisar administrar. Remova as variáveis bootstrap no Coolify depois,
sem apagar o volume. Reinícios preservam as contas e não reabrem setup.

Não há senha automática nos logs. Banco novo sem bootstrap falha fechado.
Bancos antigos mantêm admins existentes; `WM_ADMIN_PASSWORD` só permanece como
compatibilidade explícita legada, não é o caminho recomendado para servidores novos.

O painel React pode ser instalado e ativado depois pela TUI admin, sem reconstruir
o servidor; veja [painel opcional](control-panel.md) e [primeiro acesso](remote-admin.md).

## 4. Conectar as máquinas

Há dois caminhos; o convite de um comando só funciona quando o host serve o binário do cliente.

### 4.1 Convite pela interface

Após concluir o setup, use o convite inicial mostrado pelo host ou a CLI admin
para criar outros convites (consulte `wm admin --help`). Se optar por instalar e
ativar o [painel opcional](control-panel.md), abra **Tokens** e crie um convite com:

- nome/descrição para identificar a máquina;
- TTL curto (por exemplo, 1 h);
- **1 uso** por máquina;
- tags de ACL que ela deve receber (ex.: `mac`, `linux`).

O painel mostra o comando `wm join ... --token wmj_...` e, quando o binário correspondente
está disponível, o convite de um comando:

```sh
curl -fsSL https://mesh.exemplo.com/j/wmj_... | sh
```

O endpoint procura o arquivo em `/data/control/dist` dentro do container. Se o cliente for
Linux da mesma arquitetura do host, ele usa o próprio executável do container; para outras
combinações (por exemplo, Mac Apple Silicon), publique o arquivo com o nome canônico:

```sh
# no host da VPS, descubra o container do Coolify:
docker ps --filter name=wm --format '{{.ID}} {{.Names}}'

docker exec <container> mkdir -p /data/control/dist
docker cp dist/wm-darwin-arm64 <container>:/data/control/dist/wm-macos-aarch64
docker cp dist/wm-linux-arm64  <container>:/data/control/dist/wm-linux-aarch64
```

Os nomes aceitos são `wm-macos-aarch64`, `wm-macos-x86_64`, `wm-linux-x86_64` e
`wm-linux-aarch64` (também há a variante `.bin`). Sem o arquivo, use a instalação manual
abaixo.

### 4.2 Instalação manual (recomendada para Macs)

**Linux** (mesma arquitetura do host):

```sh
curl -fsSL 'https://mesh.exemplo.com/download/wm?os=linux&arch=x86_64' -o /tmp/wm
sudo install -m 0755 /tmp/wm /usr/local/bin/wm

sudo wm join https://mesh.exemplo.com --token wmj_... --name linux-01 --no-start
sudo wm up                 # teste em foreground; Ctrl+C para parar
sudo wm service install --mode agent
sudo wm service status --mode agent
```

Troque `arch=x86_64` por `aarch64` em ARM64. O endpoint só devolve o executável quando o
SO/arquitetura batem com o host; caso contrário, compile no alvo ou use um binário publicado.

**macOS** (Apple Silicon):

```sh
# use o binário do repositório (dist/wm-darwin-arm64) ou compile no Mac
sudo install -m 0755 wm-darwin-arm64 /usr/local/bin/wm

sudo wm join https://mesh.exemplo.com --token wmj_... --name mac-01 --no-start
sudo wm up
sudo wm service install --mode agent
sudo wm service status --mode agent
```

Requisitos do nó: root (TUN + rotas), `/dev/net/tun` no Linux ou `utun` no macOS. Windows
nativo não é suportado; WSL2 é experimental.

### 4.3 Verificar

No painel, o nó deve aparecer `online` e o caminho deve ser `direct` ou `relay`. No cliente:

```sh
sudo wm status
sudo wm peers
wm ping <outro-nó>
wm probe <outro-nó> 8421
```

Se o serviço real escuta só em `127.0.0.1`, reinstale o serviço com `--expose 8421`; sem isso
ele precisa escutar em `0.0.0.0` ou no IP do overlay.

## 5. ACL, tags e segurança

- A ACL **vazia é allow-all** nesta versão alpha. Antes de distribuir convites, abra **ACL** e
  crie regras restritas por tag/nome, protocolo e porta. Use o simulador do painel.
- Tags de ACL são definidas pelo administrador no convite/painel; o cliente não consegue se
  autoatribuir tags.
- Prefira um convite de **1 uso** por máquina; revogue tokens não usados.
- O nó gateway recebe a tag `gateway` automaticamente.
- Para exigir aprovação de novos nós, acrescente `--approval-required` à linha `command` do
  compose e faça o deploy. O nó gateway nasce `pending`; aprove-o no painel depois do primeiro
  start. Sem essa flag, novos nós entram ativos.
- O painel fica público na internet; use senha forte e mantenha Coolify/Docker atualizados.

## 6. Operação

### Logs

- **Runtime Logs** do Coolify mostram o control, o agente e os convites impressos no start.
- **Cuidado:** cada start do `wm host` cria e imprime um convite de 20 usos/7 dias e um convite
  rápido de 1 uso/1 h. Trate os logs como segredo e prefira revogar convites antigos no painel.
- `journalctl`/systemd não se aplicam ao container; use os logs do Coolify.

### Backup

O estado inteiro está no volume nomeado `wm-data` (SQLite, chaves, estado do agente). No host
da VPS, pare o container e copie o volume:

```sh
docker volume ls | grep wm-data
docker run --rm \
  -v <volume-wm-data>:/data:ro \
  -v "$PWD":/backup \
  alpine tar czf /backup/wm-data-$(date +%F).tgz -C /data .
```

O arquivo contém chaves privadas e tokens: guarde com a mesma sensibilidade de uma chave SSH.
Para restaurar, pare o container, extraia o conteúdo de volta no volume e suba de novo.

### Upgrade

Faça **Redeploy** no Coolify com o novo commit. O volume e o SQLite são preservados; o
container é recriado. Se uma versão futura mudar o schema, leia as notas antes — esta alpha
cria o schema no start e não tem migrações versionadas.

### Trocar domínio ou URL pública

1. Atualize o domínio no Coolify (serviço `wm`, porta `WM_HTTP_PORT`, padrão 8080).
2. Atualize `WM_PUBLIC_URL` (sem porta) e faça o deploy.
3. Crie convites novos; os antigos continuam no banco, mas apontam para a URL anterior.

Nós já inscritos guardam a URL do control em `agent-state.json`. O nó gateway do compose é
reinscrito pelo `wm host` a cada start, mas os clientes **não**: trocar o domínio exige reenrolar
cada nó ou ajustar o estado com o agente parado. Planeje o domínio antes de distribuir convites.

### Redefinir a senha

Não há CLI de reset nesta versão. As senhas permanentes são escolhidas no setup;
recolocar variáveis bootstrap ou `WM_ADMIN_PASSWORD` **não** troca senhas e não reabre
setup. Guarde as contas permanentes no seu gerenciador de senhas. Em caso de perda,
recupere um backup apropriado ou planeje recriar o volume (perdendo nós/tokens);
nunca apague dados para tentar um reset sem avaliar esse impacto.

## 7. A VPS como nó de verdade (opcional)

Como o nó do compose fica na rede bridge, a VPS não enxerga o overlay. Se você quer usar a
própria VPS para alcançar as máquinas (SSH, curl, rotear uma LAN), instale o binário no host e
rode um agente nativo **separado**, com um token de 1 uso:

```sh
# no host da VPS, fora do container
docker cp <container>:/usr/local/bin/wm /tmp/wm
sudo install -m 0755 /tmp/wm /usr/local/bin/wm

sudo wm join https://mesh.exemplo.com --token wmj_... --name vps-host --no-start
sudo wm up
sudo wm service install --mode agent --data-dir /var/lib/wired-magic
```

O default `/var/lib/wired-magic` já é diferente do `/data` do container (que fica no namespace
do próprio container). Esse agente cria o TUN no host e passa a ser um nó normal da malha; ele
recebe o próximo IP livre (normalmente `.2`), não o `.1`, que pertence ao control plane. Se o
binário do host não for compatível (por exemplo, VPS x86_64 com imagem construída em aarch64),
compile no alvo; o `docker cp` só funciona quando as arquiteturas batem.

## 8. Troubleshooting

| Sintoma | Causa provável | Ação |
| --- | --- | --- |
| Deploy falha por `WM_PUBLIC_URL` vazio ou erro de bootstrap | URL ausente ou banco novo sem credenciais temporárias | preencha `WM_PUBLIC_URL` e `WM_BOOTSTRAP_USERNAME` + senha/arquivo no Coolify; não combine com seed legado |
| Container reinicia com erro de TUN | `/dev/net/tun` ausente ou sem permissão | `ls -l /dev/net/tun`, `modprobe tun`; confira `devices`/`cap_add` |
| `operation not permitted` ao criar `wm0` | capability insuficiente no Docker | mantenha `NET_ADMIN`; em último caso, `privileged: true` (evite em produção) |
| `502` no domínio | proxy apontando para a porta errada ou container unhealthy | use `WM_HTTP_PORT` no domínio/proxy (padrão `...:8080`); veja healthcheck e logs |
| Painel abre, mas convite não carrega | `WM_PUBLIC_URL` diferente do domínio ou TLS ainda não emitido | corrija a URL, aguarde o certificado; teste `/healthz` |
| Nós não aparecem online | 443 de saída bloqueada no cliente, token inválido ou ACL | `wm doctor`; `curl https://mesh.exemplo.com/healthz` no cliente |
| Sem caminho direto, só relay | UDP configurado bloqueado, NAT/CGNAT ou firewall | libere `WM_STUN_PORT`/`WM_UDP_PORT` no host e na rede; relay é o fallback esperado |
| `address already in use` no STUN (padrão 3478) | outro STUN/serviço no host | altere `WM_STUN_PORT` e libere a nova porta UDP |
| `address already in use` no WireGuard (padrão 41641) | outro WireGuard/serviço | altere `WM_UDP_PORT` e libere a nova porta UDP |
| Convite de um comando falha no Mac | host não tem o binário macOS em `/data/control/dist` | publique o binário ou instale manualmente |

Coleta útil para abrir um issue:

```sh
docker compose logs --tail=200 wm
curl -fsS https://mesh.exemplo.com/healthz
curl -fsS https://mesh.exemplo.com/api/v1/meta
```

## 9. Limpeza

No Coolify, exclua o recurso. O volume `wm-data` pode continuar existindo:

```sh
docker volume ls | grep wm-data
docker volume rm <volume-wm-data>
```

Remova também as regras de firewall/portas publicadas que não forem mais usadas. O volume
contém chaves e o banco; apague conscientemente.

## 10. Status de validação

A versão headless/bootstrap foi validada localmente com testes Rust serializados e
smoke HTTP do binário real: setup por env, CSRF, roles, invalidação/restart, APIs de
nós/ACL e instalação/ativação/desativação/persistência do painel com fixture estática.
O React real no navegador e os builds Docker/CI não foram executados nesta refatoração.

Uma versão anterior foi exercitada em Docker Desktop Linux `arm64` (imagem,
healthcheck, painel, convites, segundo nó, caminhos direct/relay e volume persistente).
Isso **não** valida automaticamente esta versão: antes de promover, teste no builder
real a imagem headless, bootstrap pela TUI, persistência do volume, pacote opcional e
reutilização dos caches. Não houve validação em Coolify real, TLS/ACME, firewall
externo, NAT de operadora ou VPS x86_64. Trate o deploy como alpha.

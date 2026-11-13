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

- Nenhum outro processo usando `3478/udp` ou `41641/udp`. O STUN usa 3478 fixo nesta versão;
  o 41641 pode ser trocado no compose (`--udp-port` e `ports`).
- 2 vCPU e 2–4 GB de RAM são confortáveis para o build Rust + Web; em runtime o container usa
  bem menos. O build acontece no servidor do Coolify.

## 1. Variáveis de ambiente

No Coolify, crie o recurso e cadastre em **Environment Variables**:

| Variável | Obrigatória | Exemplo | Observação |
| --- | --- | --- | --- |
| `WM_PUBLIC_URL` | sim | `https://mesh.exemplo.com` | URL pública **sem** `:8080` e sem barra no final; entra no painel, nos convites e nos tokens de relay |
| `WM_ADMIN_PASSWORD` | sim | senha longa e única | só é aplicada no primeiro start; depois o hash fica no SQLite |
| `WM_NODE_NAME` | não | `gateway` | nome do nó `.1` no painel; padrão `gateway` |
| `WM_LOG` | não | `info` | `trace`, `debug`, `info`, `warn` ou `error` |

Gere a senha com algo como:

```sh
openssl rand -base64 32
```

> **Não confunda as duas URLs.** O domínio do Coolify aponta para o serviço `wm` na porta
> interna **8080** (`https://mesh.exemplo.com:8080` no campo Domains). Já `WM_PUBLIC_URL` é a
> URL pública sem porta (`https://mesh.exemplo.com`). Se colocar `:8080` nela, os convites e o
> relay passam a apontar para a porta errada.

## 2. Criar o recurso no Coolify

1. Abra o projeto/ambiente e clique em **+ New**.
2. Escolha o repositório (público ou deploy key) e a branch (`main`).
3. Em **Build pipeline**, escolha **Docker Compose**.
4. Preencha:
   - **Base Directory:** `/`
   - **Docker Compose Location:** `/docker-compose.yml`
5. Salve e abra **Domains**:
   - **Service:** `wm`
   - **Domain:** `https://mesh.exemplo.com:8080`
6. Cadastre as variáveis da seção anterior.
7. Clique em **Deploy**.

O Coolify vai construir a imagem a partir do `Dockerfile` da raiz (Web + Rust) e gerar as
labels do Traefik para o domínio. O primeiro build costuma levar alguns minutos. Um redeploy do
mesmo commit reutiliza as camadas; mudanças de código recompilam o workspace Rust.

O nó gateway se inscreve sozinho no primeiro start (`WM_NODE_NAME`, padrão `gateway`) e
reivindica `10.66.0.1`; não é preciso criar token para ele. Os tokens são para as outras
máquinas.

Se o repositório for privado, configure a deploy key no Coolify antes do deploy. O compose
usa `build.context: .`; não é necessário publicar imagem em registry.

## 3. Primeiro start e login

Acompanhe **Deployment Logs** e depois **Runtime Logs**. O healthcheck consulta
`/healthz`; quando ficar saudável:

```sh
curl -fsS https://mesh.exemplo.com/healthz
curl -fsS https://mesh.exemplo.com/api/v1/meta
```

> O primeiro start inscreve o nó gateway usando `WM_PUBLIC_URL`, ou seja, passando pelo Traefik.
> Se o DNS ou o certificado ainda não estiverem prontos, o container pode reiniciar uma ou duas
> vezes até o proxy responder; isso é esperado. O volume é preservado.

Abra `https://mesh.exemplo.com` e entre com:

- usuário: `admin`
- senha: a que você definiu em `WM_ADMIN_PASSWORD`

O painel mostra nós, convites, ACL, relay/STUN, integração, auditoria e extensões.

> Se `WM_ADMIN_PASSWORD` não for definida, o processo sorteia uma senha e a imprime **uma vez**
> no log do primeiro start. O compose exige a variável justamente para evitar perder esse
> acesso. Trocar a variável depois não muda a senha já gravada.

## 4. Conectar as máquinas

Há dois caminhos; o convite de um comando só funciona quando o host serve o binário do cliente.

### 4.1 Convite pela interface

No painel, abra **Tokens** e crie um convite com:

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

1. Atualize o domínio no Coolify (serviço `wm`, porta 8080).
2. Atualize `WM_PUBLIC_URL` (sem porta) e faça o deploy.
3. Crie convites novos; os antigos continuam no banco, mas apontam para a URL anterior.

Nós já inscritos guardam a URL do control em `agent-state.json`. O nó gateway do compose é
reinscrito pelo `wm host` a cada start, mas os clientes **não**: trocar o domínio exige reenrolar
cada nó ou ajustar o estado com o agente parado. Planeje o domínio antes de distribuir convites.

### Redefinir a senha

Não há CLI de reset nesta versão. A senha só é definida no primeiro start com
`WM_ADMIN_PASSWORD`; se ela foi perdida, resta restaurar um backup ou recriar o volume (o que
invalida nós e tokens). Guarde a senha no gerenciador de segredos do Coolify.

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
| Deploy falha por `WM_PUBLIC_URL`/`WM_ADMIN_PASSWORD` vazio | variável não cadastrada no Coolify | preencha Environment Variables e faça deploy |
| Container reinicia com erro de TUN | `/dev/net/tun` ausente ou sem permissão | `ls -l /dev/net/tun`, `modprobe tun`; confira `devices`/`cap_add` |
| `operation not permitted` ao criar `wm0` | capability insuficiente no Docker | mantenha `NET_ADMIN`; em último caso, `privileged: true` (evite em produção) |
| `502` no domínio | proxy apontando para a porta errada ou container unhealthy | domínio `...:8080`; veja healthcheck e logs |
| Painel abre, mas convite não carrega | `WM_PUBLIC_URL` diferente do domínio ou TLS ainda não emitido | corrija a URL, aguarde o certificado; teste `/healthz` |
| Nós não aparecem online | 443 de saída bloqueada no cliente, token inválido ou ACL | `wm doctor`; `curl https://mesh.exemplo.com/healthz` no cliente |
| Sem caminho direto, só relay | UDP 3478/41641 bloqueado, NAT/CGNAT ou firewall | libere UDP no host e na rede; relay é o fallback esperado |
| `address already in use` em 3478 | outro STUN/serviço no host | libere a porta; o STUN usa 3478 fixo nesta versão |
| `address already in use` em 41641 | outro WireGuard/serviço | troque `--udp-port` e o mapeamento `ports` no compose |
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

O caminho foi exercitado localmente em Docker Desktop (Linux `arm64`) com o compose deste
repositório: build da imagem, healthcheck, login no painel, criação de convite, inscrição de um
segundo nó, caminho `direct`, caminho `relay` forçado, parada graciosa com reinício e
persistência do volume. **Não** foi executado em uma instalação real de Coolify, nem validou
TLS/ACME, firewall externo, NAT de operadora ou uma VPS x86_64. Trate o deploy como alpha, como
o restante do projeto.

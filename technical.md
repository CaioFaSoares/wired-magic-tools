# wired-magic — referência técnica

Este documento guarda o que é técnico: arquitetura, protocolos, portas, segurança, build, deploy,
API, perfis, exigências por nó, testes e limites. Para a visão geral e o passo a passo de conexão,
volte ao [README](README.md).

## Visão geral

```text
                        ┌──────────────────────────────┐
                        │ wm host (VPS, nó .1)          │
                        │  axum: /api/v1 + SPA + /relay │
                        │  SQLite: nós/tokens/ACL/audit │
                        │  STUN UDP 3478                │
                        │  agente embutido: TUN .1      │
                        └───────────────┬──────────────┘
                                        │ HTTPS/WSS (saída)
              ┌─────────────────────────┼─────────────────────────┐
              ▼                         ▼                         ▼
        Mac rack 1                  Mac rack 2               container Seance
        wm join --expose 8421       wm join                   (host network/SNAT)
        TUN 10.66.0.12              TUN 10.66.0.13            10.66.0.1
              │                         │
              └──── WireGuard UDP direto (hole punching) ────────┘
                         fallback: frames WG no WSS /relay
```

O control plane **não transporta dados**. Ele autentica nós, aloca IPs, distribui um `NetworkMap`
versionado, compila ACL e assina tokens de relay. O data plane é WireGuard em userspace
(boringtun) com um TUN/utun por nó.

## Componentes e código

| Componente | Crate / pasta | Responsabilidade |
| --- | --- | --- |
| API/control | `crates/wm-control` | axum, SQLite (sqlx), presença, mapa long-poll, admin, SSE, integração |
| Relay/STUN | `crates/wm-control` | `/relay` WebSocket + STUN Binding UDP 3478 |
| Agente | `crates/wm-core` | boringtun, TUN, rotas, STUN client, candidatos, relay client, API local |
| Tipos compartilhados | `crates/wm-proto` | mapa, ACL, relay, shares, API local, convites |
| TUI | `crates/wm-tui` | console do membro: onboarding, perfis, agente, shares, console, mouse |
| Extensões | `crates/wm-ext` | contratos, validação de manifesto, cloudflared, driver `exec` |
| Binário | `crates/wm` | `host`, `serve`, `join`, `up`, `tui`, `gui`, `doctor`, `service` |
| Painel | `web/` | React + Vite, embutido no binário via `rust-embed` |

Estrutura do repositório:

```text
crates/wm-proto/    tipos compartilhados
crates/wm-core/     agente, boringtun, TUN, STUN, relay client, API local
crates/wm-ext/      manifesto de extensão, cloudflared, drivers
crates/wm-control/  axum + SQLite, relay/STUN, ACL, painel embutido
crates/wm-tui/      TUI Ratatui
crates/wm/          binário wm
web/                React + Vite
deploy/             systemd, launchd, Caddy, Docker e2e
dist/               binários de demonstração + SHA256SUMS
docs/               arquitetura, pesquisa, runbook, plano
scripts/            smoke, verificação local e e2e
```

## Como a conexão nasce

1. O agente faz **STUN** pelo mesmo socket UDP que carrega WireGuard (`0.0.0.0:41641`), senão o
   mapeamento NAT não valeria para os dados.
2. Publica `EndpointCandidate[]` (STUN, IPs LAN, anteriores) em `POST /api/v1/node/endpoints`.
3. O control entrega os candidatos de cada peer no **mapa** versionado
   (`GET /api/v1/map?version=N&wait=25`, long-poll de até 25 s).
4. Cada peer dispara handshake WireGuard para os candidatos, em ordem STUN → LAN → anterior. Um
   pacote WG válido de volta fixa o endpoint e o caminho fica `direct`.
5. Sem handshake em ~5 s, os pacotes vão em frames binários pelo WebSocket TLS `/relay`. O relay
   só vê ciphertext WireGuard; não decifra nem forja.
6. A cada 30 s o agente tenta voltar ao caminho direto. A sessão WireGuard é a mesma; a troca de
   transporte não derruba conexões estabelecidas.

Detalhes de implementação: [ARCHITECTURE](docs/ARCHITECTURE.md),
[ADR 0002](docs/adr/0002-control-plane-map.md), [ADR 0003](docs/adr/0003-relay-transport.md).

### Presença e control fora do ar

`last_seen` sobe em cada request autenticado; online = visto nos últimos 90 s ou conectado ao
relay. Relatórios de path/RTT e presença **não** incrementam a versão do mapa, para evitar
tempestade de long-polls. Control fora do ar **não derruba** túneis já estabelecidos; o mapa volta
quando o control responde.

## Overlay, nomes e ACL

- Overlay padrão `10.66.0.0/24` (configurável), host `.1`, peers `.2`–`.254`, MTU 1280.
- O endereço é estável; serviços e regras não dependem do IP público do nó.
- Nomes existem no mapa/CLI/API (`wm resolve <nome>`); não há DNS do sistema no v1.
- **ACL** em YAML: `action`, `src`, `dst`, `protocol`, `ports`, `comment`.
  - Seletores: `*`, `name:x`, `tag:y`, IP/CIDR, expandidos no save.
  - Ordem first-match; sem regra que case → `default_allow` (true quando a política está vazia).
  - A mesma `CompiledAcl` vai no mapa; cada agente avalia **egress e ingress**.
  - Uma tabela de fluxos (5 min) permite respostas de conexões estabelecidas sem exigir regra
    espelhada para portas efêmeras.
  - Limite de 8192 regras compiladas.

## Identidade, inscrição e convites

- O agente gera **X25519** localmente. A privada fica em `agent-state.json` (0600); a pública vai
  no `POST /api/v1/enroll`.
- O control guarda só a pública, o IP e metadados. Um token de nó opaco (`wmn_...`, hash SHA-256
  no banco) autentica as chamadas seguintes.
- Join tokens (`wmj_...`) têm TTL, limite de usos e tags; são consumidos no enroll.
- O código `wmc_...` é opaco e carrega URL + token; o `wm join` e a TUI aceitam
  `wmc_...`, `wm://join...`, ou URL + token.
- O host no primeiro start cria:
  - **convite rápido**: 1 uso, ~1 h, impresso como código `wmc_...` e como comando
    `curl .../j/<token> | sh`;
  - **convite manual**: 20 usos, 7 dias, impresso como `wm join ... --token wmj_...`.
- `--approval-required` (host) ou `require_approval` no convite coloca novos nós em `pending` até
  um admin aprovar.
- Admin único: senha argon2id, cookie HttpOnly, CSRF nas mutações.

### Identidade de exibição

O nome técnico do nó é a identidade da ACL e **não muda** ao personalizar a apresentação. Ao lado
dele existe o perfil de exibição (`display_name` + `about`), que é o que os outros membros veem nos
painéis, no mapa e na TUI.

```sh
wm join https://mesh.exemplo --token wmj_... --display-name "Caio" --about "rack da sala"
wm host --display-name "Estúdio" --about "host da equipe"
# depois: TUI → Início → p (Meu perfil) ou PATCH /api/v1/node/profile
```

## Perfis e várias redes

Uma instalação pode participar de **várias malhas ao mesmo tempo**. Cada rede é um perfil com
data-dir, inscrição, agente, socket, log e identidade próprios.

- A raiz do `--data-dir` guarda `profiles.json` e o diretório `profiles/`.
- A instalação antiga continua sendo o perfil `default` (mesma pasta de antes).
- Cada agente concorrente precisa de **TUN e porta UDP distintos**.
- A TUI avisa (`⚠`) quando duas redes ativas usam o mesmo CIDR de overlay, o que faria as rotas
  colidirem; configure overlays diferentes ou mantenha só uma ativa.

```sh
sudo wm --data-dir /usr/local/var/wired-magic tui                    # perfil ativo
sudo wm --data-dir /usr/local/var/wired-magic tui --profile amigos   # abre/cria "amigos"
wm --data-dir /usr/local/var/wired-magic/profiles/amigos status      # cada perfil é um data-dir
```

Na TUI, a aba **Redes** lista as inscrições, alterna a rede ativa e inicia/para agentes sem
derrubar os outros. `Enter` troca a rede ativa, `n` cria outra, `s`/`x` inicia/para, `r` renomeia
o rótulo local, `d` remove (só sem agente ativo).

## Exigências por nó

O administrador do control plane pode anexar exigências a cada nó. O agente reporta o que
realmente está rodando a cada 30 s e o servidor calcula a conformidade; o painel web
(**Nós → editar → Exigências**) e a TUI (aba **Início**) mostram o resultado. Não conformidade é
**sinalizada** nesta versão, nunca desconecta o nó silenciosamente.

Campos aceitos:

- `min_version` — versão mínima do `wm`, comparada com semver quando possível;
- `required_extensions` — extensões que precisam estar ativas e saudáveis;
- `required_tags` — tags que o nó precisa carregar (definidas pelo admin/convite);
- `agent` — `udp_port`, `mtu`, `force_relay`, `expose`, `tun_name`.

```sh
# painel web: Nós → lápis → Exigências do servidor
curl -X PUT https://mesh/api/v1/admin/nodes/<id>/requirements \
  -H 'Content-Type: application/json' -b cookie -H "X-WM-CSRF: $csrf" \
  -d '{"min_version":"0.2.0","required_extensions":["cloudflared"],
       "required_tags":["mac"],"agent":{"mtu":1400}}'
```

Convites continuam sendo o caminho padrão: quem tem o convite entra. `require_approval` no
convite coloca os nós daquele convite em `pending` até um admin aprovar.

## Segurança

- WireGuard ponta a ponta; relay e control nunca veem o tráfego; o control guarda só chaves
  públicas.
- Convites com TTL/uso controlado; token do nó revogável; relay token assinado (Ed25519) com
  audiência.
- ACL aplicada nos dois lados; **default aberto quando a política está vazia**. Endureça antes de
  produção; o simulador do painel mostra a regra que casou.
- O agente roda como root (TUN/rotas); o socket local é `0660`. Não há login de aluno.
- `--expose` amplia a superfície do nó; exponha só a porta necessária e mantenha a ACL restrita.
- `wm host` sem TLS é só para loopback/LAN. Para produção, use Caddy/Let's Encrypt ou
  `--tls-cert`/`--tls-key`, e nunca exponha o data dir/SQLite.

Modelo de ameaças, revogação e hardening: [SECURITY](docs/SECURITY.md).

## Portas e firewall

| Porta | Protocolo | Para quê |
| --- | --- | --- |
| 443 | TCP | painel, API e relay WSS (pelo proxy TLS) |
| 3478 | UDP | STUN |
| 41641 | UDP | WireGuard/STUN dos nós (recomendada; entrada opcional) |

O host precisa de 443/tcp e 3478/udp acessíveis, mais 41641/udp para o caminho direto. Os nós só
fazem conexões de saída; não precisam de port forwarding. Sem IP público, clientes fora da LAN não
alcançam o host: o control precisa ser acessível por HTTPS e o STUN por UDP.

Portas personalizadas:

```sh
sudo wm host --listen 0.0.0.0:8443 \
  --public-url https://mesh.exemplo.com:8443 \
  --tls-cert /etc/wired-magic/cert.pem --tls-key /etc/wired-magic/key.pem \
  --stun-listen 0.0.0.0:5349 --udp-port 51820
```

Libere **8443/tcp**, **5349/udp** e **51820/udp**. API, painel e relay compartilham a porta
HTTP/TLS. Com proxy reverso, use a URL/porta pública do proxy em `--public-url`; a porta de
`--listen` é interna. Se houver remapeamento UDP para outra porta pública, anuncie-a com
`--stun-url stun:mesh.exemplo.com:PORTA` (repetível).

Variáveis úteis: `WM_DATA_DIR`, `WM_PUBLIC_URL`, `WM_ADMIN_PASSWORD` (só no primeiro start),
`WM_LISTEN`, `WM_STUN_LISTEN`, `WM_STUN_URLS`, `WM_UDP_PORT`, `WM_LOG`.

## Instalar e compilar

### Homebrew (macOS)

```sh
brew tap CaioFaSoares/tap
brew install CaioFaSoares/tap/wired-magic
```

A release atual é `v0.1.3`. A TUI instalada pelo Homebrew verifica e instala versões estáveis mais
novas em segundo plano; a sessão e o agente continuam na versão atual e a nova entra na próxima
abertura. `WM_NO_AUTO_UPDATE=1` desativa. Fórmulas fixadas com `brew pin` são respeitadas; sob
`sudo`, a TUI apenas orienta a atualizar como usuário normal (nunca executa Homebrew como root).
Detalhes: [homebrew](docs/homebrew.md).

### Binários de demonstração

`dist/` traz macOS Apple Silicon e Linux aarch64 (glibc), com `SHA256SUMS`. Não são releases
assinadas/notarizadas; em macOS o binário tem assinatura ad-hoc. Em macOS Intel ou Linux x86_64,
compile.

### Compilar da fonte

Requisitos:

- Rust **1.94+** e um linker C (Xcode Command Line Tools no macOS; `build-essential` no Linux).
- Node.js **22 LTS** (mínimo 20.19) e **pnpm 10** para o painel.
- Linux: `iproute2` em runtime e `systemd` se for instalar o serviço.

```sh
# 1. painel (necessário para o painel embutido atualizado)
cd web
pnpm install --frozen-lockfile
pnpm build
cd ..

# 2. binário (rust-embed embute web/dist em tempo de compilação)
cargo build --release -p wired-magic

# 3. instale antes de instalar o serviço
sudo install -m 0755 target/release/wm /usr/local/bin/wm
wm version
```

Se mudar o frontend depois de compilar o Rust, recompile o Rust: o painel é embutido no binário.
Um build só do CLI pode usar o `web/dist` já presente no repositório.

### Cross-compile e Docker

```sh
# Linux nativo (mesma arquitetura):
make linux

# x86_64 em máquina ARM:
docker run --rm --platform linux/amd64 \
  -v "$PWD":/src -w /src \
  -v "$PWD/.cargo-home":/cargo \
  -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/src/target-linux \
  rust:1.98-bookworm cargo build --release -p wired-magic
```

O resultado fica em `target-linux/release/wm`. Para musl/Alpine, compile com o triple
`*-unknown-linux-musl`; o binário glibc não roda em musl.

### Publicar binários para o convite

O endpoint `/download/wm` procura o diretório `dist` do control:

- `wm host`: `<data-dir>/control/dist/` (ex.: `/var/lib/wired-magic/control/dist/`);
- `wm serve`: `<data-dir>/dist/`.

Nomes canônicos: `wm-macos-aarch64`, `wm-macos-x86_64`, `wm-linux-x86_64`, `wm-linux-aarch64`.
O host usa o próprio executável quando o cliente tem o mesmo SO/arquitetura, mas uma VPS Linux
não serve binário macOS. Para frotas mistas, publique os quatro arquivos.

## Hospedar

O host é o único lugar com endereço público. Ele concentra control plane, relay, STUN e, por
padrão, o nó `.1` (gateway). Use uma VPS Linux para produção; um Mac funciona para laboratório,
mas precisa ficar ligado e sem dormir.

### Linux (recomendado)

```sh
sudo mkdir -p /var/lib/wired-magic
sudo wm host \
  --public-url https://mesh.seudominio.com \
  --listen 127.0.0.1:8080 \
  --data-dir /var/lib/wired-magic
```

Primeiro start imprime a senha do `admin`, um convite rápido `wmc_...`, o comando
`curl .../j/... | sh` e um convite manual. Anote a senha: só o hash fica no SQLite.

- `--public-url` é a URL que clientes e painel usam; entra nos tokens e nos convites.
- `--listen 127.0.0.1:8080` deixa o TLS para o Caddy; para TLS direto use
  `--listen 0.0.0.0:443 --tls-cert ... --tls-key ...`.
- `--approval-required` faz novos nós entrarem como `pending`.
- `--tag`/`--label` do host são administrativos; o nó local recebe a tag `gateway`.
- `--expose 8421` publica um serviço local do host no overlay.

TLS na frente (Caddy):

```caddyfile
mesh.seudominio.com {
    encode zstd gzip
    reverse_proxy 127.0.0.1:8080 {
        transport http {
            read_timeout 90s
            write_timeout 90s
        }
    }
}
```

O proxy precisa repassar WebSocket em `/relay` e não pode bufferizar o long-poll/SSE. Os timeouts
de 90 s existem por causa do mapa (25 s) e dos keepalives do SSE.

Serviço systemd:

```sh
sudo wm service install --mode host \
  --public-url https://mesh.seudominio.com \
  --listen 127.0.0.1:8080 \
  --data-dir /var/lib/wired-magic
sudo wm service status --mode host
journalctl -u wired-magic-host -f
```

O gerador escreve `/etc/systemd/system/wired-magic-host.service` com argumentos escapados e não
usa shell. As opções precisam ser repetidas no `service install`; ele não herda o comando
foreground. Para mudar opções: `wm service uninstall --mode host` e reinstale.

Verificação:

```sh
curl -fsS https://mesh.seudominio.com/healthz
curl -fsS https://mesh.seudominio.com/api/v1/meta
sudo wm status
sudo wm peers
```

Upgrade/backup: pare o serviço, substitua `/usr/local/bin/wm`, reinicie; o data dir (incluindo
`<data-dir>/control/control.db` e as chaves) é preservado. Para backup, pare o serviço e copie o
data dir inteiro; SQLite e chaves são sensíveis.

### macOS

```sh
sudo wm host \
  --public-url https://mesh.seudominio.com \
  --listen 127.0.0.1:8080 \
  --data-dir /usr/local/var/wired-magic

sudo wm service install --mode host \
  --public-url https://mesh.seudominio.com \
  --listen 127.0.0.1:8080 \
  --data-dir /usr/local/var/wired-magic
sudo wm service status --mode host
sudo tail -f /var/log/wired-magic-host.log
```

O launchd cria `/Library/LaunchDaemons/com.wired-magic.host.plist` e inicia como root no boot. O
macOS pode pedir permissão de firewall; permita o `wm`. O TUN é `utun` e o número é descoberto pelo
agente.

### Sem TLS (dev/LAN)

Use HTTP somente em loopback ou numa rede confiável. **Nunca exponha isso à internet.**

```sh
# terminal 1 — só control plane, sem TUN e sem root (porta > 1024)
wm serve --listen 127.0.0.1:8080 \
  --public-url http://127.0.0.1:8080 \
  --data-dir ./wm-data

# terminal 2 — um nó nesta máquina (root; um agente por máquina)
sudo wm join http://127.0.0.1:8080 --token wmj_... --name dev --no-start
sudo wm up
```

`wm host` também aceita `--public-url http://127.0.0.1:8080`, mas já sobe o nó local e precisa de
root. Para apenas control/relay/STUN sem TUN, use `wm serve` ou `wm host --no-agent`.

> **Não misture `wm host` e `wm serve` no mesmo data dir.** O `wm host` guarda o control em
> `<data-dir>/control`; o `wm serve` usa o data dir diretamente. Use diretórios separados.

### Coolify / Docker Compose

A raiz tem `Dockerfile` e `docker-compose.yml` para o host completo. O Coolify cuida do TLS e do
domínio; o compose publica só as portas UDP que o proxy HTTP não cobre. Pré-requisitos: VPS Linux
com Docker rootful, `/dev/net/tun` no host e 443/tcp, 3478/udp e 41641/udp liberadas.

```sh
cp .env.example .env      # edite WM_PUBLIC_URL e WM_ADMIN_PASSWORD
docker compose up -d --build
docker compose logs -f wm
```

O compose não publica `8080/tcp` de propósito: em produção quem fala com o container é o proxy.
Para testar localmente sem proxy, use um override que publique `127.0.0.1:8080:8080` e ajuste
`WM_PUBLIC_URL` para `http://127.0.0.1:8080`; não deixe essa porta aberta na internet.

Guia detalhado, backup e troubleshooting: [DEPLOY-COOLIFY](docs/DEPLOY-COOLIFY.md).

## Entrar como nó

O `--name` precisa ser único na malha; sem ele, o hostname da máquina é usado. As tags de ACL são
definidas pelo administrador no convite/painel; `--tag` enviado pelo cliente não concede
autoridade. `--code` (`wmc_...`) carrega URL e convite juntos; `--token` vem de um convite manual.

### macOS

```sh
sudo wm join https://mesh.seudominio.com --token wmj_... --name mac-01 --no-start
sudo wm up                     # teste em foreground; Ctrl+C para parar

sudo wm service install --mode agent --data-dir /usr/local/var/wired-magic
sudo wm service status --mode agent
sudo tail -f /var/log/wired-magic-agent.log
```

O `service install` captura `--control`, `--tun-name`, `--force-relay`, `--udp-port`, `--mtu` e
`--expose`; repita ali qualquer opção que precise sobreviver ao boot.

### Linux

```sh
sudo apt update && sudo apt install -y iproute2 curl ca-certificates   # Debian/Ubuntu
ls -l /dev/net/tun             # se não existir, o kernel/container não tem TUN

sudo wm join https://mesh.seudominio.com --token wmj_... --name linux-01 --no-start
sudo wm up
sudo wm service install --mode agent --data-dir /var/lib/wired-magic
journalctl -u wired-magic-agent -f
```

Se a máquina não usa systemd (container, Alpine/OpenRC), rode `sudo wm up` em foreground ou
escreva a unit do seu init; `wm service install` só gera systemd no Linux.

### Windows via WSL2 (experimental)

Não existe cliente Windows nativo: o agente usa TUN, rotas do sistema e socket Unix. WSL2 executa
o binário Linux, mas **apps Windows e jogos nativos não veem** `10.66.0.0/24`.

```powershell
wsl --install -d Ubuntu-24.04
```

Dentro do Ubuntu: instale `iproute2`/`curl`, confira `/dev/net/tun`, habilite systemd no
`/etc/wsl.conf` se quiser `wm service install`, e siga o fluxo Linux. O NAT do WSL2 pode impedir o
caminho direto; o relay é o fallback esperado. Para testar de forma determinística, use
`--force-relay`. Não recomendamos rodar o host no WSL2.

### Containers

Um nó avulso em container precisa de `/dev/net/tun` e `NET_ADMIN`; sem isso o agente não cria a
interface. O e2e do repositório usa exatamente esse caminho:

```sh
make linux                 # gera target-linux/release/wm
scripts/e2e-docker.sh      # control + relay/STUN + 3 nós Linux privilegiados
```

No Docker Desktop do macOS, containers privilegiados podem servir para testes, mas **não** dão
overlay aos apps do macOS. Para um nó de verdade, prefira uma VM Linux.

> **Uma máquina, um agente.** As rotas do overlay são da máquina inteira. Para simular dois nós no
> mesmo computador, use duas VMs/containers — não dois `wm join` no mesmo host. Perfis permitem
> várias redes, mas cada agente precisa de TUN e porta UDP distintos.

## Shares

Um share é um serviço TCP/UDP publicado por um nó com nome único na malha. O endereço é sempre o
IP do overlay do dono + a porta; o tráfego passa pelo WireGuard (direto ou relay), então **não há
porta pública no nó**.

```sh
# no nó que hospeda; o serviço precisa escutar em 0.0.0.0 (docker -p já faz)
docker run --rm -p 3000:3000 minha-app
wm share 3000 --name demo --kind web --description "preview do app"

# em outro nó
wm shares
wm open demo              # abre http://10.66.0.x:3000
wm open demo --print      # só imprime o endereço
```

Tipos: `web`, `tcp`, `udp`, `game` (TCP+UDP). O share `game` imprime o endereço para
`steam://connect/...`. A GUI local publica, renomeia, liga/desliga e remove shares; a TUI faz o
mesmo na aba **Shares**.

`--expose` no `service install` publica um serviço que escuta só em loopback no IP do overlay do
próprio nó (porta overlay → porta local). A opção do `wm join`/`wm up` vale apenas para aquela
execução; repita no `service install` para persistir. Sem `--expose`, o serviço precisa escutar em
`0.0.0.0` ou no IP do overlay.

## TUI, GUI e CLI

### TUI

```sh
sudo wm tui [--profile <slug>] [--no-auto-start] [--no-mouse]
```

Abas: **Início**, **Redes**, **Peers**, **Shares**, **Mapa**, **Agente**, **Extensões**,
**Serviços**, **Console** e **Ajuda** (`1`…`9`; a décima e as abas de extensões com `Tab`/setas).
Extensões podem declarar uma aba própria com `[tui]` (`snapshot` ou `exec`); `chat` e `debug` já
fazem isso.

- **Entrar sem sair:** sem inscrição, a aba **Início** mostra o onboarding; `j` abre o formulário
  para colar `wmc_...` ou informar URL + token. A TUI inscreve, inicia o agente e mantém a conexão.
- **`q` não derruba a malha:** o agente é iniciado numa sessão própria (`setsid`), com `stdin`
  fechado e saída em `<data-dir>/agent.log`. `q`/Ctrl+C fecha só a tela; o processo continua e a
  próxima TUI reconecta. Para parar: `X` na aba Agente, `:down` ou `wm down`.
- **Auto-start:** com a inscrição salva, abrir a TUI inicia o agente se ele estiver parado
  (`--no-auto-start` desliga por uma sessão). Parar pelo `X` desliga o auto-start; `s` inicia e
  religa.
- **Sessão persistente:** aba, histórico, configurações, jobs e o último retrato de peers/shares
  ficam em `<data-dir>/tui-state.json` (0600); eventos em `tui-events.jsonl`. Tokens e convites são
  redigidos antes de gravar.

Teclas principais:

| Onde | Teclas |
| --- | --- |
| Início / Agente | `s` inicia, `X` para, `R` reinicia, `e` ajustes, `a` auto-start, `l` log, `d` doctor, `i` serviço, `g` GUI, `p` Meu perfil |
| Redes | `Enter` troca, `n` cria, `s`/`x` inicia/para, `r` renomeia, `d` remove, `p` perfil |
| Peers | `p` ping, `o` probe, `x` reconecta, `c` copia IP, `n` renomeia o nó |
| Shares | `n` publica, `Enter` abre, `c` copia, `r` renomeia, `e` on/off, `d` remove (duas vezes) |
| Serviços | `Enter` log, `g`/`G` GUI, `p` painel, `x` para job, `i`/`u` instala/remove serviço, `t` status, `d` doctor |
| Global | `:` console, `?` ajuda, `m` menu, `Tab`/setas abas, `q` sai |

Mouse: abas, botões, linhas e modais são clicáveis; duplo clique abre share/probe/log; roda rola;
clique direito (ou `m`) abre o menu. `--no-mouse` desliga a captura (útil para selecionar texto).

### GUI local

```sh
sudo wm gui                 # abre o navegador
sudo wm gui --no-open       # imprime a URL privada
sudo wm gui --listen 127.0.0.1:9000
```

A GUI sobe **somente em loopback** (`127.0.0.1:8787` por padrão) e entrega a capacidade privada no
fragmento `#t=...`. Não exige senha de admin; usa o token do nó no socket local. Serve para ver o
estado, peers e shares, entrar na malha com um código, publicar/gerenciar shares e abrir
`steam://connect/...`. Não compartilhe a URL com `#t=...`: ela vale enquanto o processo estiver
ativo. Bind fora de loopback é rejeitado.

### Comandos

| Comando | Função |
| --- | --- |
| `wm host` | control + relay + STUN + agente local + convites |
| `wm serve` | só control plane (aceita `--config`) |
| `wm join` | inscreve (ou inicia, se já inscrito) e roda o agente |
| `wm up` / `wm down` | sobe/para o agente já inscrito em foreground |
| `wm status [--json]` | estado do nó |
| `wm peers [--json]` | peers, caminho, RTT, bytes |
| `wm ping <nó>` | ICMP pelo overlay |
| `wm probe <nó> <porta>` | TCP pelo overlay |
| `wm resolve <nome>` | resolve o nome no mapa |
| `wm share <porta> --name <nome>` | publica um serviço (`--kind` web, game, tcp ou udp) |
| `wm shares [--json]` | lista os shares da malha |
| `wm extensions [--json]` | catálogo de extensões deste nó |
| `wm extensions enable <id>` / `disable <id>` | ativa/desativa no nó atual |
| `wm chat --url <url>` | TUI de chat conectada a um `chat_gui` |
| `wm chat_gui [--listen]` | serve a interface web do chat |
| `wm debug` | dashboard TUI local de performance |
| `wm debug_gui [--listen]` | painel web de performance |
| `wm packages <subcomando>` | extensões de repositórios git (discover/install/update/remove) |
| `wm open <nome> [--print]` | abre um share web ou imprime o endereço |
| `wm share-rename`, `wm share-enable`, `wm unshare` | gerenciam shares do nó |
| `wm rename <nome>` | renomeia este nó |
| `wm gui` | GUI local de membro (`--no-open`, `--listen`) |
| `wm tui` | console completo (`--profile`, `--no-auto-start`, `--no-mouse`) |
| `wm doctor` | diagnóstico (root, control, rota, socket) |
| `wm version` | versão e protocolo |
| `wm service install/status/uninstall` | systemd/launchd (`--mode agent` ou `host`) |

`--data-dir` e `--log` são globais e podem aparecer antes ou depois do subcomando. O mesmo agente
só pode rodar uma vez por máquina; `wm service install` recusa uma segunda instalação de modo
diferente até você remover a primeira.

## Painel web, API e integração

O painel é servido na `public-url` do host, junto com a API. Nele você:

- aprova/revoga nós e vê o estado online e o caminho (direto/relay);
- cria convites com tags, TTL, limite de usos e `require_approval`;
- edita a ACL e usa o simulador de regras;
- configura relay/STUN e vê a topologia;
- define exigências por nó e vê a conformidade;
- cria service tokens para integração e consulta a auditoria;
- gerencia extensões, como o Cloudflare Tunnel.

Login: `admin` + a senha do primeiro start (ou `WM_ADMIN_PASSWORD` no primeiro start). Sessão
cookie HttpOnly de 12 h; mutações exigem CSRF. Não existe login de aluno.

Integração somente leitura:

```http
GET /api/v1/integration/nodes
Authorization: Bearer wms_...
```

Resposta: `{nodes:[{id,name,hostname,overlay_ip,tags,labels,online,last_seen}], server_time}`.
Crie o service token em **Integração** no painel; o escopo é `integration:nodes:read`. Contrato
completo: [API](docs/API.md); consumo pelo Seance: [INTEGRATION-SEANCE](docs/INTEGRATION-SEANCE.md).

## Extensões e hooks

Manifestos ficam em `<data-dir>/extensions/<id>/extension.toml`; extensões declarativas usam
`node_driver = "exec"` e um JSON validado por nó. O servidor pode **solicitar** uma extensão para
um nó; o pedido aparece no catálogo com `!`, mas o membro decide ativar. A aba **Extensões** da TUI
mostra o catálogo do control e um catálogo local, visível mesmo com o agente parado. Extensões
podem declarar uma aba própria com `[tui]` (`snapshot` ou `exec`); `chat` e `debug` já fazem isso.

Pacotes também podem ser instalados de repositórios git no formato flake (`wm packages`). Hooks e
adapters estão em [hooks-adapters](docs/hooks-adapters.md); o guia geral é
[extensions](docs/extensions.md). O alvo do cloudflared precisa ser loopback do próprio nó;
credenciais nunca voltam pela API.

## Testes e verificação

```sh
# sem root, sem Docker, sem TUN: typecheck/build Web, testes Rust, contratos E2E e smoke HTTP
bash scripts/verify-local.sh

# testes Rust
cargo test --workspace

# smoke HTTP do control com o binário real (não usa TUN)
cargo build -p wired-magic
python3 scripts/smoke-local.py --binary target/debug/wm

# E2E privilegiado em Docker (Linux; exige target-linux/release/wm)
make linux
scripts/e2e-docker.sh
```

O `verify-local.sh` guarda logs e um `manifest.json` com hashes em `evidence/local/run.*`; não
substitui fmt/lint/auditoria de dependências. O E2E sobe control + relay/STUN + 3 nós Linux
privilegiados e cobre inscrição, caminho direto, `--expose`, relay determinístico
(`--force-relay`), ACL restrita (TCP/ICMP com conntrack), control fora e API de integração.

O que esses testes **não** provam: dataplane em TUN real, DOM de navegador, reboot de
systemd/launchd, NAT de campo e jogos reais. Detalhes em [TESTE-LOCAL](docs/TESTE-LOCAL.md),
[E2E-ASSERTIVO](docs/E2E-ASSERTIVO.md) e [SERVICOS-TESTE](docs/SERVICOS-TESTE.md).

## Limites do v1

Linux/macOS nativos; **sem Windows nativo**, iOS/Android, exit node, subnet router, HA de control,
multi-região de relay, ACME embutido, SSO/OIDC e DNS do sistema (há nomes no mapa/CLI/API). É um
POC operacional, não um produto auditado. Um host é um ponto central de disponibilidade; o relay
é um fallback, não uma promessa de caminho direto. O caminho direto depende de NAT/UDP e pode cair
para relay de forma esperada.

A pergunta “Windows é necessário para os jogadores?” está aberta no plano de prontidão. Se for, o
escopo precisa ser reaberto e implementado/testado; WSL2 não substitui um cliente Windows para
apps nativos.

## Documentos

| Documento | Conteúdo |
| --- | --- |
| [PLANO-PRONTIDAO](docs/PLANO-PRONTIDAO.md) | gates G0–G5, cenários de campo e o que falta para o aceite |
| [IMPLEMENTACAO-STATUS](docs/IMPLEMENTACAO-STATUS.md) | o que foi implementado, verificado e pendente |
| [TESTE-LOCAL](docs/TESTE-LOCAL.md) | verificação local sem privilégios |
| [E2E-ASSERTIVO](docs/E2E-ASSERTIVO.md) | E2E Docker, assertions e cobertura |
| [SERVICOS-TESTE](docs/SERVICOS-TESTE.md) | instalação de systemd/launchd e migração |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | componentes, protocolos e limites |
| [RESEARCH](docs/RESEARCH.md) | como uma malha estilo Tailscale funciona |
| [SECURITY](docs/SECURITY.md) | modelo de ameaças, credenciais, ACL e lacunas |
| [DEPLOY-VPS](docs/DEPLOY-VPS.md) | VPS, Caddy, systemd, Seance e operação |
| [DEPLOY-COOLIFY](docs/DEPLOY-COOLIFY.md) | compose no Coolify, portas, backup e troubleshooting |
| [RUNBOOK-TAUA](docs/RUNBOOK-TAUA.md) | cenário de laboratório Tauá |
| [API](docs/API.md) | endpoints, formatos e erros |
| [INTEGRATION-SEANCE](docs/INTEGRATION-SEANCE.md) | leitura de nós pelo Seance |
| [SHARES](docs/SHARES.md) | contrato de shares, GUI/TUI e loopback |
| [extensions](docs/extensions.md) | extensões, ativação por nó, cloudflared e drivers |
| [hooks-adapters](docs/hooks-adapters.md) | hooks de ciclo de vida, adapters e receita Seance |
| [TROUBLESHOOTING](docs/TROUBLESHOOTING.md) | diagnóstico e correções por sintoma |
| [benchmarks](docs/benchmarks.md) | medições e ambiente de referência |
| [deploy/README.md](deploy/README.md) | Caddy, systemd, launchd, Docker/Seance e SNAT |

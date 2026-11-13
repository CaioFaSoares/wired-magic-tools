# Shares — expor um serviço local na malha

Um **share** é um serviço TCP/UDP que um nó publica na malha com um nome
estável. É a camada que transforma a VPN em uma ferramenta de demonstração e de
jogo: o dono roda o serviço onde quiser (inclusive `docker run -p ...`) e os
outros nós o encontram pelo nome, não pelo IP.

```
dev A (10.66.0.5)                         dev B (10.66.0.7)
┌─────────────────────┐                   ┌─────────────────────┐
│ docker run -p 3000  │                   │ wm gui              │
│ wm share 3000       │  mapa de shares   │  wm://demo          │
│   --name demo       │ ────────────────► │  http://10.66.0.5:3000
└─────────────────────┘                   └─────────────────────┘
```

## Contrato

Um share é único na malha pelo `name` (slug `[a-z0-9][a-z0-9-]*`, 1–63).
Campos:

| campo | significado |
| --- | --- |
| `name` | nome curto usado em `wm://name` |
| `node_id` | nó dono; o endereço de rede é o overlay IP desse nó |
| `port` | porta local publicada pelo serviço |
| `kind` | `web`, `tcp`, `udp` ou `game` (apresentação no painel) |
| `protocol` | `tcp`, `udp` ou `both` |
| `description` | texto livre curto |
| `enabled` | desligado some do acesso, mas continua no inventário |
| `public_url` | reservado para a URL do cloudflared (a extensão gerencia a rota) |

O nó dono cria/edita/remove os próprios shares pela API autenticada pelo token
do nó (`/api/v1/node/shares`). O admin vê tudo em `/api/v1/admin/shares`.

O mapa (`/api/v1/map`) carrega `shares` para todos os peers. Cada agente
resolve `wm://name` localmente, sem uma chamada extra ao control plane.

## Endereço

- `web`: `http://<overlay-ip>:<port>/`
- `tcp` / `game` com TCP: `<overlay-ip>:<port>`
- `udp` / `game` com UDP: `<overlay-ip>:<port>` (mesmo formato; o cliente
  escolhe o transporte)

O serviço precisa escutar em `0.0.0.0` (ou no IP do overlay) para que o tráfego
que chega pelo TUN seja aceito. `docker run -p 3000:3000` já faz isso. Servidores
de desenvolvimento presos em `127.0.0.1` podem ser encaminhados com
`wm up --expose 3000` (o agente escuta no IP do overlay e projeta para o
loopback).

## CLI

```sh
wm share 3000 --name demo --kind web --description "preview do app"
wm shares
wm share-rename demo demo-v2
wm share-enable demo --off
wm share-enable demo
wm unshare demo
wm open demo                 # abre o navegador para shares web
wm open demo --print         # só imprime o endereço
wm rename dev-a              # renomeia este nó (mapa e painel)
```

## TUI

A aba **Shares** lista todos os shares da malha. Nos shares do nó local:

- `n` publica uma porta local (formulário com porta, nome, tipo, transporte e descrição);
- `r` renomeia;
- `e` liga/desliga;
- `d` remove (pressione `d` de novo para confirmar);
- `Enter` abre (web) ou copia/mostra o endereço (tcp/udp/game);
- `c` copia o endereço selecionado.

Na aba **Peers**, `n` renomeia o nó local. A TUI mantém o agente rodando depois de `q` e mostra
o último retrato de shares quando ele está parado; publicar, renomear ou remover exige o agente
ativo. O comando `:share 3000 --name demo` também publica pelo console da TUI.

## GUI local (`wm gui`)

`wm gui --open` sobe uma página somente em loopback (`127.0.0.1:8787` por
padrão) para o membro da malha, sem exigir login de admin. A capacidade privada
vai no fragmento `#t=...` e é removida da URL pelo cliente após armazená-la na
sessão da aba. O HTML público não contém esse segredo. Sem `--open`, a URL
privada é mostrada diretamente no terminal quando disponível, não nos logs
stdout/stderr. Em ambiente sem terminal, use `--open`. Não compartilhe a URL;
a capacidade vale enquanto o processo estiver ativo. Bind fora de loopback é
rejeitado, e Host/Origin são verificados.

Funcionalidades:

- estado do agente, peers e shares;
- caixa “conectar com um código” (`wmc_...`) quando ainda não há estado;
- cards dos shares com **Abrir** (web), **Copiar endereço** e **Steam** (jogo);
- formulário para publicar uma porta local;
- renomear/ligar/desligar/remover os próprios shares.

## Onboarding

`wm host` imprime:

```sh
curl -fsSL https://mesh.exemplo.com/j/wmj_... | sh
```

O endpoint devolve um script que baixa o binário do próprio control
(`/download/wm`), instala e roda `wm join` com um código `wmc_...` embutido. O
código é opaco (`wmc_` + base64url de URL + token) e o token só existe hasheado
no SQLite.

## Cloudflared

A extensão **Cloudflare Tunnel** (página *Extensões* do painel) roda no nó e
publica rotas para serviços locais:

- modo `quick`: uma URL efêmera `https://...trycloudflare.com`, sem conta;
- modo `named`: um túnel com credenciais e hostnames do seu domínio, por
  exemplo `wired.seudominio.com` apontando para `http://127.0.0.1:3000`.

O share continua sendo o diretório interno (`wm://demo`); a rota do cloudflared
é a porta pública. Hoje a rota é criada no painel de extensões, não pelo
`wm share`.

## Limites

- O share não muda a ACL: se a ACL bloquear o nó, o share não abre.
- `wm://` é um esquema de resolução da ferramenta; não é registrado no sistema
  operacional. A GUI e a CLI aceitam `wm://name`.

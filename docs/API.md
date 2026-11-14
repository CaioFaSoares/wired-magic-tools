# API

Prefixo: `/api/v1`. JSON em UTF-8. Erros:

```json
{"error":{"code":"...","message":"...","details":null}}
```

Autenticação:
- **Nó**: `Authorization: Bearer wmn_...`
- **Admin**: cookie `wm_session` (HttpOnly) + header `X-WM-CSRF` nas mutações
- **Integração**: `Authorization: Bearer wms_...` com escopo `integration:nodes:read`
- **Público**: `/healthz`, `/api/v1/meta`, `/api/v1/enroll`

## Público

### `GET /healthz`

```json
{"ok":true,"version":"0.1.0","protocol":1,"uptime_secs":42}
```

### `GET /api/v1/meta`

Versão, chave pública do control, rede, URLs de relay/STUN.

### `POST /api/v1/enroll`

```json
{
  "join_token":"wmj_...", "name":"mac-01", "hostname":"mac.local",
  "os":"macos", "arch":"aarch64", "version":"0.1.0",
  "wg_public_key":"base64...", "labels":{},"tags":[],"nat_type":"unknown"
}
```

Resposta: `node_id`, `node_token` (mostrado uma vez), `overlay_ip`, `overlay_cidr`,
`node_name`, `network_name`, `control_pubkey`.

## Nó

### `GET /api/v1/map?version=N&wait=25`

Mapa completo. Se `N` é a versão atual, segura até 25 s. Resposta:

```json
{
  "version":1,"protocol":1,"network_name":"wired-magic",
  "overlay_cidr":"10.66.0.0/24","dns_zone":"wm.internal","mtu":1280,
  "me":{"id":"...","name":"site","overlay_ip":"10.66.0.1","wg_public_key":"...","dns_name":"site.wm.internal","tags":[],"labels":{}},
  "peers":[{"id":"...","name":"mac-01","overlay_ip":"10.66.0.12","wg_public_key":"...","endpoints":[{"addr":"203.0.113.9:41641","kind":"stun"}],"online":true,"last_seen":"...","tags":["mac"],"labels":{}}],
  "relay":{"url":"https://mesh.exemplo.com/relay","region":"vps","token":"...","expires_at":"..."},
  "stun":["stun:mesh.exemplo.com:3478"],
  "acl":{"version":1,"default_allow":false,"rules":[...]},
  "extensions":[],
  "server_time":"..."
}
```

### `POST /api/v1/node/endpoints`

```json
{"udp_port":41641,"candidates":[{"addr":"203.0.113.9:41641","kind":"stun"}],"nat_type":"cone","relay_connected":true}
```

### `POST /api/v1/node/report`

`peers[]` com path/rtt/handshake/bytes, `nat_type`, `relay_connected`, `extensions[]`.
Não incrementa a versão do mapa.

### `POST /api/v1/node/leave`

Marca o nó como desabilitado e remove a conexão do relay.

## Admin

### `POST /api/v1/admin/login`

```json
{"username":"admin","password":"..."}
```

Resposta: `{user, csrf_token}` + `Set-Cookie: wm_session=...`. Guarde o CSRF e envie
`X-WM-CSRF` em POST/PUT/PATCH/DELETE.

### `POST /api/v1/admin/logout` (CSRF)

### `GET /api/v1/admin/me`

### `GET /api/v1/admin/overview`

Contagens de nós, online/pendentes, pares direct/relay, relay conectado, uptime.

### `GET /api/v1/admin/nodes`

Lista de `NodeView` (PeerInfo + status, created/approved/disabled, nat, relay, reported_peers).

### `PATCH /api/v1/admin/nodes/{id}` (CSRF)

```json
{"name":"mac-01","tags":["mac"],"labels":{"baguette_port":"8421"},"status":"active"}
```

Recompila a ACL ativa quando nome/tags/status mudam.

### `POST /api/v1/admin/nodes/{id}/approve` (CSRF)

### `DELETE /api/v1/admin/nodes/{id}` (CSRF)

Revoga: status disabled, tokens revogados, conexão de relay removida, ACL recompilada.

### Join tokens

- `GET /api/v1/admin/join-tokens`
- `POST` (CSRF): `{"name":"lab","ttl_secs":3600,"max_uses":10,"tags":["mac"]}` → token em claro
  **uma vez**.
- `DELETE /api/v1/admin/join-tokens/{id}` (CSRF)

### ACL

- `GET /api/v1/admin/acl` → `{policy, compiled, updated_at}`
- `PUT /api/v1/admin/acl` (CSRF): `{"yaml":"version: 1\nrules: [...]"}`; valida, compila e
  publica nova versão.
- `POST /api/v1/admin/acl/validate`: `{"yaml":"..."}` → policy, compiled, warnings.
- `POST /api/v1/admin/acl/simulate`: `{"src":"site","dst":"mac-01","protocol":"tcp","port":8421}`
  → `{allowed, rule_index, reason}`.

Seletores: `*`, `name:x`, `tag:y`, IP/CIDR. Protocolo: `tcp`, `udp`, `icmp`, `*`. Portas:
`22`, `8000-8100`, `*`. A primeira regra que casa vence; sem regra, `default_allow`.

### Eventos e auditoria

- `GET /api/v1/admin/events` — SSE `{type,data}` (node_updated, presence, acl_updated…).
- `GET /api/v1/admin/audit?limit=200` — eventos de auditoria.

### Service tokens

- `GET /api/v1/admin/service-tokens`
- `POST` (CSRF): `{"name":"seance","scopes":["integration:nodes:read"]}` → token uma vez.
- `DELETE /api/v1/admin/service-tokens/{id}` (CSRF)

### `GET /api/v1/admin/settings`

Rede, approval_required, relay/STUN, protocolo e versão.

## Integração

### `GET /api/v1/integration/nodes[?tag=mac&online=true]`

```json
{"nodes":[{"id":"...","name":"mac-01","hostname":"mac.local","overlay_ip":"10.66.0.12",
"tags":["mac"],"labels":{"baguette_port":"8421"},"online":true,"last_seen":"..."}],
"server_time":"..."}
```

### `GET /api/v1/integration/nodes/{name}`

Um nó; 404 se inexistente/inativo.

### `GET /api/v1/integration/health`

Contagens para monitoração.

## Shares

Um share é um serviço TCP/UDP com nome único na malha. O nó dono gerencia os
próprios shares com o token de nó (sem senha de admin); o admin vê todos.

Payload de criação:

```json
{"name":"demo","port":3000,"kind":"web","protocol":"tcp",
 "description":"preview","enabled":true}
```

`kind`: `web`, `game`, `tcp`, `udp`. `protocol`: `tcp`, `udp`, `both`; `web` é
sempre TCP e `game` sem protocolo vira `both`.

Autenticado pelo token do nó (`Authorization: Bearer wmn_...`):

- `GET /api/v1/node/shares` — shares do próprio nó, inclusive desligados.
- `POST /api/v1/node/shares` — cria; `409 share_name_taken` se o nome existe.
- `PATCH /api/v1/node/shares/{id}` — campos parciais (`name`, `port`, `kind`,
  `protocol`, `description`, `enabled`); só o dono.
- `DELETE /api/v1/node/shares/{id}` — só o dono.
- `PATCH /api/v1/node/self` — `{"name":"novo-nome"}` renomeia o nó.

Admin (cookie + CSRF nas mutações):

- `GET /api/v1/admin/shares` — todos os shares com nó, IP, tipo e estado.
- `POST /api/v1/admin/shares` — `{"node_id":"...","name":...}` (campos de criação
  no mesmo objeto).
- `PATCH|DELETE /api/v1/admin/shares/{id}`.

`NetworkMap.shares` leva um `ShareInfo` por share de nó ativo, inclusive
desligados (`enabled=false`), para a TUI/GUI do dono poder religá-los. O agente
resolve `wm://nome` localmente e não precisa de uma chamada extra ao control.

## Extensões

O estado desejado viaja em `NetworkMap.extensions`; o observado volta em
`ReportRequest.extensions`. `NetworkMap.exposes` leva portas de adapters
(`MeshExpose{overlay_port,local_port}`). O contrato completo, ativação por nó,
hooks e adapters estão em [`extensions.md`](extensions.md) e
[`hooks-adapters.md`](hooks-adapters.md).

- `GET /api/v1/admin/extensions` — manifestos carregados + contagens; cada
  item inclui `visibility` (`public`/`private`) e `builtin`.
- `GET /api/v1/admin/extensions/{id}` — manifesto + config/status por nó.
- `POST /api/v1/admin/extensions/reload` (CSRF) — relê manifestos; incrementa a
  versão do mapa.
- `PUT /api/v1/admin/extensions/{id}/nodes/{node_id}/activation` (CSRF) —
  `{"active":true|false}` no nó.
- `PUT /api/v1/admin/extensions/{id}/nodes/{node_id}/request` (CSRF) —
  `{"requested":true|false,"message":"..."}`; o pedido aparece no catálogo do
  nó, mas não ativa a extensão.
- `GET /api/v1/node/extensions` — catálogo para a TUI/GUI local (token do nó);
  cada item inclui `requested`, `request_message` e `tui` (a aba declarada pela
  extensão, quando houver).
- `PUT /api/v1/node/extensions/{id}` — ativa/desativa no nó atual; `{"active":bool}`.
- `GET|PUT|DELETE /api/v1/admin/extensions/{id}/nodes/{node_id}/config` (CSRF
  nas mutações) — config genérica de extensões declarativas.
- `GET /api/v1/admin/extensions/cloudflared/overview` — nós, rotas e status.
- `GET|POST /api/v1/admin/extensions/cloudflared/routes`,
  `PUT|DELETE .../routes/{id}` — rotas por projeto.
- `GET /api/v1/admin/extensions/cloudflared/nodes`,
  `PUT .../nodes/{node_id}`, `POST .../nodes/{node_id}/restart` — túnel,
  credenciais e restart.
- `GET /api/v1/node/shares`, `POST/DELETE` — shares do próprio nó (ver
  [`SHARES.md`](SHARES.md)); o mapa leva o diretório para todos os agentes.

## Relay

`GET /relay` (WebSocket) com `Authorization: Bearer <relay token>`; frames binários definidos
em `wm_proto::RelayFrame`. Não é uma API HTTP pública.

## Limites

- Sem paginação em nodes/audit (limite 1000 no audit).
- Sem OpenAPI servido; este documento e `crates/wm-proto` são o contrato.
- Extensões: ver [`extensions.md`](extensions.md); `extensions`/`shares` no mapa são opcionais
  para clientes antigos (`serde(default)`).

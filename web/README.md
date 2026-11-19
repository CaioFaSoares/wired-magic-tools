# wired-magic — painel web

Painel de gestão do control plane da malha, em português. Stack: Vite + React 19 +
TypeScript + Tailwind CSS 4 + React Router + TanStack Query. Não há login de aluno,
terminal ou xterm neste escopo.

O servidor padrão é **headless**: não compila nem embute React. A saída em
`web/dist` pode ser empacotada como extensão opcional `control_panel`, instalada no
servidor e ativada por admin com confirmação do digest, sem recompilar Rust.
Veja [pacote, ativação e modelo de confiança](../docs/control-panel.md).
Depois da ativação, `index.html` é servido na raiz com fallback SPA; namespaces da
API/relay/download continuam reservados. JavaScript do pacote roda na mesma origem
com a autoridade do operador autenticado: a extensão não é sandbox.

## Requisitos

- Node.js >= 20.19 (testado com 26.12.1)
- pnpm 10 (o repositório traz `pnpm-lock.yaml`)

## Desenvolvimento

```bash
cd web
pnpm install
pnpm dev
```

- Painel: <http://127.0.0.1:5173>
- Proxy de dev: `/api` → `http://127.0.0.1:8080`
- Para apontar para outro control sem editar arquivo:
  `WM_PROXY_TARGET=http://127.0.0.1:18081 pnpm dev`

O proxy usa `selfHandleResponse` para descarregar os headers do SSE imediatamente.
Sem isso, o Node segura os headers até o primeiro keepalive (15 s) e o indicador
“ao vivo” parece travado em “conectando”.

## Login

Use a **conta admin permanente** escolhida no [setup inicial pela TUI](../docs/remote-admin.md).
Não há senha gerada/impressa nem setup pelo React. Bootstrap e conta cotidiana não
administram este painel; conclua o setup antes de ativá-lo. Bancos legados mantêm
seus admins anteriores.

- A sessão fica em cookie **HttpOnly / SameSite=Lax** (12 h).
- O token **CSRF** é guardado em `sessionStorage` (somente a aba atual) e enviado
  em `X-WM-CSRF` nas mutações.
- Na abertura do painel, `GET /api/v1/admin/session` recupera usuário + CSRF para
  uma aba nova. Se o control for antigo e não tiver essa rota, o cliente cai para
  `GET /api/v1/admin/me`; nesse caso, sem CSRF na aba, a leitura funciona e as
  mutações pedem um novo login.
- `401` volta ao login; `403 csrf_missing/csrf_invalid` mostra o aviso de CSRF.

## Mock opt-in

Para desenvolver a UI sem backend:

```bash
cd web
VITE_MOCK=1 pnpm dev
```

Use **admin / admin**. O mock cobre login, nós, tokens, ACL, relay/STUN,
service tokens, integração, auditoria e settings em memória; reinicia no reload e
não é instalado fora de `VITE_MOCK=1` (o build normal não carrega o arquivo de mock).

> Não gere build de produção com `VITE_MOCK=1`.

## Build

```bash
cd web
pnpm typecheck
pnpm build
```

Gera:

```text
web/dist/index.html
web/dist/assets/index-<hash>.css
web/dist/assets/index-<hash>.js
web/dist/favicon.svg
```

O JS é dividido em chunks de vendor (`react`, `query`, `yaml`). Sem CDN, sem fontes
externas e sem scripts inline; o CSP do backend permite apenas a própria origem.
Na raiz do repositório, empacote com
`python3 scripts/package_control_panel.py --output dist/control_panel.tar.gz`.
O script não sobrescreve saídas existentes; escolha um novo nome para uma versão nova.
Instale e confirme o digest seguindo [o guia do painel](../docs/control-panel.md).
Não é necessário reconstruir ou reiniciar o binário Rust.

Scripts: `dev`, `build`, `preview`, `typecheck`.

## Páginas e endpoints

Prefixo fixo: `/api/v1`. Toda chamada admin usa o cookie; mutações exigem
`X-WM-CSRF`.

| Página / componente | Rotas usadas |
| --- | --- |
| Login | `GET /meta`, `POST /admin/login`, `GET /admin/session` (fallback `GET /admin/me`), `POST /admin/logout` |
| Shell / sidebar | `GET /meta`, `GET /admin/overview`, `GET /admin/session`, `GET /admin/events` (SSE) |
| Visão geral (`/overview`) | `GET /admin/overview`, `GET /admin/nodes`, `GET /admin/settings`, `GET /meta` |
| Nós (`/nodes`) | `GET /admin/nodes`, `POST /admin/nodes/{id}/approve`, `PATCH /admin/nodes/{id}`, `DELETE /admin/nodes/{id}` |
| Tokens (`/tokens`) | `GET/POST /admin/join-tokens`, `DELETE /admin/join-tokens/{id}` |
| ACL (`/acl`) | `GET/PUT /admin/acl`, `POST /admin/acl/validate`, `POST /admin/acl/simulate`, `GET /admin/nodes` |
| Relay e STUN (`/relay-status`; `/relay` é transporte WSS) | `GET /admin/settings`, `GET /meta`, `GET /admin/overview`, `GET /admin/nodes` |
| Integração (`/integration`) | `GET/POST /admin/service-tokens`, `DELETE /admin/service-tokens/{id}`, `GET /integration/nodes` (teste com Bearer) |
| Auditoria (`/audit`) | `GET /admin/audit?limit=N`, `GET /admin/events` (invalidação) |
| Configurações (`/settings`) | `GET /admin/settings`, `GET /meta` |
| Topologia | Componente SVG próprio, sem endpoint extra; usa `nodes` e `overview` |

O SSE emite eventos nomeados `event: wm` com `{type, data}`. O cliente escuta `wm`
(e também `message`) e invalida somente as queries afetadas. Tipos de evento:
`node_updated`, `node_removed`, `join_token_created`, `join_token_revoked`,
`acl_updated`, `relay_updated`, `audit`, `presence` e `extension_status`.

## Pendências e observações do contrato

1. **ACL sem YAML bruto.** `GET /admin/acl` devolve `policy` tipada, não o texto YAML
   original. O editor serializa `AclView.policy` de volta para YAML; comentários e
   formatação originais não são preservados. `PUT` grava o novo YAML.
2. **Simulador usa a ACL salva.** `POST /admin/acl/simulate` compila/consulta o que
   está no banco, não o rascunho do editor. A UI avisa isso e exige salvar antes de
   simular um rascunho.
3. **Settings somente leitura.** Não existe `PUT/PATCH /admin/settings`; a tela
   exibe os valores efetivos e aponta para as flags/`WM_*` do processo.
4. **Service tokens revogados.** `ServiceTokenView` não expõe `revoked_at`, mas
   `GET /admin/service-tokens` continua listando revogados. A UI os esconde apenas
   na sessão corrente, depois de revogar; após reload eles reaparecem sem indicar
   o estado. Revogar novamente é inofensivo. Um campo `revoked_at` resolveria.
5. **STUN sem health.** Não há endpoint de saúde de STUN; a página mostra as URLs
   anunciadas em `/admin/settings` e `/meta`. O estado vivo disponível é o número
   de conexões do relay (`overview.relay_connected`) e o relatório dos nós.
6. **`default_allow` da ACL.** O compilador faz `default_allow = rules.is_empty()`.
   Uma política com seletores que não resolvem pode compilar para zero regras e
   continuar aberta, apesar de ter regras no YAML. A UI mostra exatamente o
   `CompiledAcl` retornado; warnings de validação indicam seletores ignorados.
7. **Online do nó.** `NodeView.online` reflete apenas `last_seen`; o overview também
   soma presença via relay. A UI usa `online || relay_connected` e rotula “via relay”
   para não divergir do card do overview.
8. **SSE `join_token_created` carrega o token bruto** no payload do evento. O painel
   ignora o payload e apenas invalida a lista, para não exibir o segredo fora da
   resposta de criação. Idealmente o evento não carregaria o token.
9. **Auditoria sem paginação.** O endpoint aceita apenas `limit` (1–1000); a tela
   oferece 100/200/500/1000, sem cursor.
10. **Compatibilidade de CSRF.** `/admin/session` é a fonte de CSRF ao reabrir uma
    aba; em versões anteriores do control a rota não existe e vale o fallback para
    `/admin/me` descrito acima.

As páginas de extensões/cloudflared presentes em `src/pages/` são trabalho paralelo
no mesmo diretório e não fazem parte do escopo original deste painel.

# ADR 0002 — Control plane em axum/SQLite e mapa por long-poll

- **Status:** Aceito; implementado
- **Data:** 2026-11-12
- **Decisores:** dono + pesquisa técnica
- **Relacionado:** [RESEARCH §3](../RESEARCH.md#3-control-plane-e-mapa-por-long-poll),
  [ADR 0004](0004-overlay-addressing.md)

## Contexto

O control plane precisa matricular nós, alocar IPs, distribuir chaves públicas, candidatos de
endpoint, ACL compilada e tokens de relay para até 250 nós. A operação é uma pessoa e um VPS;
não há orçamento de complexidade para Redis, NATS, etcd, Postgres ou um serviço separado de
coordenação.

O `wm-proto` define `NetworkMap`, `PeerInfo`, `RelayInfo`, `CompiledAcl` e `EnrollRequest`.
O `wm-control` implementa os handlers HTTP, o builder do mapa, o long-poll, o relay, o STUN, o
admin e a integração; o `wm-core` consome `/api/v1/map` a cada ciclo.

## Decisão

1. **axum 0.8 + SQLite (sqlx 0.9, WAL)** em um único binário. Schema em `db.rs`: `nodes`,
   `node_tokens`, `join_tokens`, `acl_policies`, `service_tokens`, `audit_events`,
   `peer_stats`, `admin_users`, `admin_sessions`, `settings`.
2. **Mapa completo e versionado em memória.** `AppState.version` é um `watch<u64>`
   inicializado de `settings.map_version` (ou 1) e incrementado por `bump_version()` nas
   mutações que mudam o mapa. A versão é persistida no SQLite **no start**, não a cada mutação;
   o mapa é substituído inteiro pelo agente, não é um delta.
3. **`GET /api/v1/map?version=N&wait=25`.** Se `N != current_version`, responde imediatamente;
   se `N == current_version`, espera até `wait` (default 25, clamp 1..25) por uma mudança de
   versão e então responde o mapa completo. **Sempre 200**, não há `204`/ETag no v1.
4. **Presença derivada, fora da versão.** `last_seen_at` é atualizado em qualquer request
   autenticado; `online` é `now - last_seen <= 90 s` (`ONLINE_WINDOW_SECS` em `db.rs`) ou o
   relay estar conectado. Heartbeat não incrementa a versão do mapa; um loop de presença a
   cada 10 s emite `ControlEvent::Presence` quando o estado muda.
5. **Autenticação do nó por bearer token** (`node_token`, hash SHA-256 no banco, revogável).
   O mapa só inclui nós `active`; `pending`/`disabled` ficam na API administrativa; o endpoint
   do mapa devolve `pending_approval` para nó pendente.
6. **Token de relay no mapa**, com `expires_at` de **1 h** no código atual. Cada `build_map`
   assina um token novo, então o agente recebe um token renovado a cada ciclo de 25 s; não há
   endpoint separado de refresh.
7. **Reconexão com backoff exponencial de 1 s a 30 s** no cliente atual; jitter é recomendado,
   mas ainda não aplicado.

## Por que 25 s e 90 s

- 25 s fica abaixo de timeouts comuns de proxy/LB (30 s é conservador), então a conexão não é
  cortada por idle timeout.
- 250 nós fazem ~10 req/s no total (não 250 req/s), e o `watch` entrega mudanças assim que a
  versão muda; no pior caso, o cliente espera até 25 s.
- 90 s = três janelas de 25 s + 15 s de margem; tolera um poll perdido, jitter e proxy lento.
- O timeout não é o caminho de atualização; é o limite da conexão ociosa. O custo é que o mapa
  completo volta a cada 25 s mesmo sem mudança.

## Alternativas

| Alternativa | Prós | Contras | Veredito |
| --- | --- | --- | --- |
| **Long-poll HTTP** | simples, auth por header, passa por proxy, request/response versionado | uma conexão por nó; precisa timeout >25 s no proxy | **escolhido** |
| WebSocket de controle | push bidirecional, baixa latência | estado de conexão, reconexão, auth de upgrade, mais código | rejeitado no v1 (o relay já é WSS) |
| SSE | uma via, simples | não tem request/response por versão; auth/retry próprios | rejeitado |
| Polling fixo (1–5 s) | trivial | latência/tráfego; 250 nós × 1 s = 250 req/s | rejeitado |
| Delta map | payload menor | estado diferencial, ordenação, recuperação de perda | rejeitado no v1; medir antes |
| Redis/NATS/etcd | fanout/HA | mais um serviço e mais operação | rejeitado |
| Postgres | robustez/HA | ops e backup maiores que o problema | rejeitado no v1 |

## Consequências

- O proxy reverso precisa de `proxy_read_timeout`/idle > 30 s, sem buffering; HTTP/1.1 ou h2.
- O payload do mapa é O(n) e é devolvido a cada 25 s por nó; 250 nós exigem medição de
  tamanho, build e fanout. Se doer, o próximo passo é 304/ETag + mapa sob demanda, ou delta —
  não começar por delta.
- SQLite tem um writer; `last_seen` a cada 25 s por nó gera ~10 req/s e ~20 UPDATEs/s com
  250 nós (nó + token). Medir; se necessário, coalescer `last_seen`.
- Sem HA: uma instância. O data plane direto sobrevive à queda; novas descobertas e mudanças
  de ACL param. Isso é aceitável no v1 e explícito em [RESEARCH §10](../RESEARCH.md#10-revogação-e-tolerância-a-falhas).
- Um restore de backup pode regredir a versão; como o mapa é completo, o agente aplica mesmo
  assim e registra o evento.

## Critérios de aceite

- Dois agentes convergem para um nó novo assim que o `watch` acorda; ≤ 25 s no pior caso.
- 250 clientes de teste mantêm long-polls sem estourar file descriptors/CPU.
- Um heartbeat de 25 s não muda `map.version`; o mapa completo volta, mas o build é O(n).
- O agente aceita o mapa completo mesmo com versão menor (restart/restore).
- Reinício do control plane: agentes reconectam com backoff, sem pico > 2× o normal.
- `set_node_status(disabled)` remove o nó do mapa e revoga os node tokens.

## Referências

- Código: [`crates/wm-control/src/db.rs`](../../crates/wm-control/src/db.rs),
  [`crates/wm-control/src/state.rs`](../../crates/wm-control/src/state.rs),
  [`crates/wm-control/src/api/node.rs`](../../crates/wm-control/src/api/node.rs),
  [`crates/wm-core/src/control.rs`](../../crates/wm-core/src/control.rs)
- Tipos: [`crates/wm-proto/src/lib.rs`](../../crates/wm-proto/src/lib.rs)
- Tailscale `/map` long-poll (referência de comportamento):
  <https://github.com/tailscale/tailscale/blob/main/control/controlclient/direct.go>
- Tailscale — control/data planes e cache offline:
  <https://tailscale.com/docs/concepts/control-data-planes>

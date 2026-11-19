# Arquitetura

## 1. Visão geral

```text
                        ┌──────────────────────────────┐
                        │ wm host (VPS, nó .1)          │
                        │ axum: /api/v1 + /relay        │
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

O control plane **não transporta dados**. Ele autentica nós, aloca IPs, distribui um
`NetworkMap` versionado, compila ACL e assina tokens de relay. O data plane é WireGuard
userspace no espaço do usuário, com um TUN/utun por nó.

## 2. Componentes

| Componente | Crate | Responsabilidade |
|---|---|---|
| API/control | `wm-control` | axum, SQLite (sqlx), presença, mapa long-poll, admin, SSE, integração |
| Relay/STUN | `wm-control` | `/relay` WebSocket + STUN Binding UDP 3478 |
| Agente | `wm-core` | boringtun, TUN, rotas, STUN client, candidatos, relay client, API local |
| TUI | `wm-tui` | console do membro; onboarding, agente destacado, shares, console, mouse/atalhos e sessão persistente |
| CLI | `wm` | host/join/up/down/serve/tui/doctor/service |
| Front opcional | `web/` | pacote `control_panel` separado, instalado e ativado por admin em runtime; nenhum build/asset React obrigatório no servidor |

## 3. Identidade e matrícula

- O agente gera **X25519** localmente (`StaticSecret`). A privada fica em
  `agent-state.json` (0600); a pública vai no `POST /api/v1/enroll`.
- O control guarda só a pública, o IP e metadados. Um token de nó opaco
  (`wmn_...`, hash SHA-256 no banco) autentica as chamadas seguintes.
- Join tokens (`wmj_...`) têm TTL, limite de usos e tags; são consumidos no enroll.
- Contas humanas: roles `admin`/`user`, senha argon2id + cookie HttpOnly/SameSite=Lax +
  CSRF. Conta humana não é nó e usuário comum não recebe privilégios administrativos.
- Servidor novo: credencial bootstrap via env, sessão `wm_setup` de 10 minutos e
  criação atômica de admin + usuário pela TUI. Conclusão consome setup e revoga todas
  as sessões bootstrap; reinício não reabre. [Setup](remote-admin.md).
- Painel React é [extensão opcional do servidor](control-panel.md), sem Node ou
  assets no build padrão; confiança por digest, snapshot verificado em memória e
  entrega bloqueada durante setup. JavaScript same-origin é código confiável, não sandbox.
- Relay tokens são assinados com Ed25519 do control e têm audiência = URL do relay.

## 4. Mapa e presença

`GET /api/v1/map?version=N&wait=25` com Bearer do nó:

- Se `N` é a versão atual, o handler segura até 25 s (watch de versão) e devolve o snapshot.
- O snapshot é **completo**: eu, peers ativos (IP, chave pública, endpoints, tags, labels),
  relay/STUN, ACL compilada, MTU, zona DNS.
- Versão sobe em: enroll, aprovação/revogação, mudança de endpoints, ACL, tokens.
- Relatórios de path/RTT e presença **não** sobem versão; evitam tempestade de long-polls.
- `last_seen` é atualizado em cada request autenticado; online = visto nos últimos 90 s ou
  conectado ao relay.
- O agente mantém o último mapa em memória; control fora não derruba túneis.

## 5. Endpoints e NAT traversal

1. O agente faz STUN Binding pelo **mesmo socket UDP** que carrega WireGuard
   (`0.0.0.0:41641`), senão o mapeamento não valeria para os dados.
2. Publica `EndpointCandidate[]` (STUN, IPs LAN, anteriores) em
   `POST /api/v1/node/endpoints`; o control incrementa a versão se mudou.
3. Com o mapa, cada peer inicia handshake WG para até todos os candidatos (lista pequena,
   deduplicada, ordenada STUN→LAN→anterior). Pacote WG válido de volta fixa o endpoint.
4. Sem handshake em ~5 s, o agente usa o relay (se conectado e com presença do peer).
5. A cada 30 s, peers relayados tentam de novo o caminho direto; a sessão WG não muda,
   só o transporte.
6. Keepalive WG 25 s mantém o mapeamento NAT. Mudança de rede → novo STUN + candidatos.

O handshake init não tem receiver index; o agente tenta o peer do cache de endpoint e
depois os outros (O(n), n ≤ 254 no /24). Pacotes de dados são O(1) por
`receiver_idx >> 8`, pois cada `Tunn` recebe um seed de índice único por peer.

## 6. Data plane

- `Tunn` do boringtun por peer, `persistent_keepalive = 25 s`, MTU 1280.
- TUN L3 criado com `tun`; rotas:
  - Linux: `ip addr replace <ip>/32 dev tunN`, `ip route replace 10.66.0.0/24 dev tunN`.
  - macOS: `ifconfig utunN inet <ip> <ip> up`, `route -n add -net 10.66.0.0/24 -interface utunN`.
- Leitura do TUN → ACL de saída → `encapsulate` → UDP direto ou frame de relay.
- UDP/relay → ACK de sessão → ACL de entrada → escrita no TUN.
- ACL é aplicada nos dois lados; uma tabela de fluxos (5 min) permite respostas de
  conexões estabelecidas (TCP/UDP) sem exigir regra espelhada para portas efêmeras.
- ICMP echo é interceptado para o `wm ping`; respostas não vão ao kernel.
- Forward `--expose 8421` cria um listener no IP overlay e projeta para
  `127.0.0.1:8421`; o SYN passa pela ACL antes de chegar ao kernel/serviço.

## 7. Relay WSS

- Um WebSocket TLS por nó em `wss://<host>/relay`, autenticado por Bearer assinado.
- Frames binários: `Data{dst,payload}`, `Deliver{src,payload}`, `Presence`, `Ping/Pong`, `Error`.
- O relay mantém `DashMap<NodeId, mpsc::Sender>`; entrega com `try_send` e descarta sob
  backpressure (o WG reenvia/retransmite). Só vê ciphertext.
- Presença do relay é separada da presença do control: um peer pode estar online no mapa e
  ainda não ter conectado ao relay.
- Revogação remove a conexão do hub; o writer fecha o socket. Tokens expiram em 1 h.

## 8. ACL

- YAML: `action`, `src`, `dst`, `protocol`, `ports`, `comment`.
- Seletores: `*`, `name:x`, `tag:y`, IP/CIDR; expandidos no save para regras `/32` e CIDRs.
- Ordem first-match; sem regra que case → `default_allow` (true se a política não tem regras).
- A mesma `CompiledAcl` vai no mapa; cada agente avalia egress e ingress.
- Mudança de tag/nome/status de nó recompila a política ativa.
- Limite de 8192 regras compiladas.

## 9. Estado e concorrência

- Agente: uma task de dados (comandos via `mpsc`) serializa peers/crypto; tasks separadas
  para TUN read, UDP recv, timer, mapa, relatório, discovery, relay e API local.
- Snapshot para a API local (`RwLock`) é reconstruído a cada 1 s ou em mudança relevante.
- Control: SQLite WAL, até 8 conexões; watch channel para versão; broadcast para SSE.
- Relay: DashMap + canais por conexão.

## 10. Limites conhecidos do v1

- Handshake init é O(n) por tentativa; dados são O(1). Para 254 nós é aceitável.
- Sem conntrack completo: a tabela de fluxos cobre TCP/UDP estabelecidos, não ICMP genérico
  nem fragmentos.
- Sem DNS do sistema; nomes existem no mapa, `wm resolve`, painel e API.
- Relay único e sem HA; sem multi-região.
- Sem subnet router/exit node; sem IPv6 no overlay.
- A exposição de loopback é allowlisted e manual (`--expose`), não automática.
- O control é fonte de verdade única; backup do SQLite é obrigatório.

# wired-magic — pesquisa técnica de uma VPN malha estilo Tailscale

**Data:** 2026-11-12
**Atualização pós-implementação (2026-11-12):** o agente passou a ter **forward allowlisted**
(`--expose overlay:local`, ex.: Baguette 8421), **tabela de fluxos** para respostas TCP/UDP e
ICMP, recompilação da ACL quando tags/nome/status mudam e remoção da conexão de relay na
revogação. As seções 2.1, 5.2 e 9 foram escritas antes disso; onde falam “não implementado”,
leia “implementado no agente, com os limites descritos em ARCHITECTURE/SECURITY”.
**Escopo:** investigação de desenho. Não é material de marketing e não contém números de
benchmark. Onde não há medição, está escrito **a medir**, com o método em
[`benchmarks.md`](benchmarks.md).

Este documento descreve como uma VPN malha funciona, o desenho do wired-magic, por que ele é
rápido, claro e funcional para o caso do Seance, e o que foi adotado ou rejeitado de
Tailscale/Headscale, NetBird e Nebula. O código existente é citado; onde a implementação ainda
não existe, o texto descreve o contrato a implementar, não um comportamento observado (§1.3).

**Documentos relacionados:** [ADR 0001 — data plane](adr/0001-data-plane-boringtun.md) ·
[ADR 0002 — control plane](adr/0002-control-plane-map.md) ·
[ADR 0003 — relay](adr/0003-relay-transport.md) ·
[ADR 0004 — endereçamento](adr/0004-overlay-addressing.md) ·
[ADR 0005 — ACL e integração](adr/0005-acl-and-integration.md) ·
[benchmarks](benchmarks.md)

---

## 0. Resumo em uma página

| Pergunta | Resposta curta |
| --- | --- |
| O que é a malha? | Uma rede L3 privada sobre a internet. Cada nó recebe um IP fixo em `10.66.0.0/24` e fala com os outros por WireGuard, diretamente quando possível e por relay quando não. |
| Quem coordena? | Um control plane em um VPS público: matrícula, mapa de peers, chaves públicas, candidatos de endpoint, ACL compilada e tokens de relay. Ele não vê o tráfego. |
| Como os nós se encontram? | STUN descobre o endpoint público; o control plane distribui candidatos; os agentes fazem hole punching por UDP; se falhar, o relay WSS na 443 encaminha os pacotes WireGuard. |
| Por que é rápido? | Caminho direto como padrão; demux O(1) no data path por `receiver_idx >> 8`; uma `Tunn` do boringtun por peer; ACL compilada; relay com frames binários, sem JSON/base64 por pacote. |
| Por que é claro? | Um `NetworkMap`, um `RelayFrame`, uma `CompiledAcl`, um SQLite, um binário de controle. Sem service mesh, sem banco externo, sem protocolo proprietário de dados. |
| Por que é funcional? | Funciona atrás de NAT, tem fallback por 443, roda em Linux e macOS, sobrevive à queda do control plane para o tráfego já estabelecido e integra com o Seance por uma API read-only. |
| O que ainda falta? | Handshake init O(1), classificação de NAT com dois servidores, rendezvous explícito (hoje os dois lados iniciam no mapa/timer) e as medições de produção. O forward do Baguette (`--expose`) e o conntrack TCP/UDP/ICMP foram implementados depois deste texto; ver a nota de atualização e `ARCHITECTURE.md`. |

**Não-objetivos do v1:** cliente para aluno, exit node, subnet router, acesso móvel, SSO/OIDC,
multi-tenant, HA do control plane, IPv6 no overlay, resolver DNS completo, módulo de kernel.

---

## 1. Contexto e escopo

### 1.1 O modelo de uma VPN malha

Uma VPN malha estilo Tailscale tem cinco peças:

1. **Identidade e matrícula.** Cada nó prova que pode entrar na rede (token de convite, SSO ou
   certificado) e publica uma chave pública. A chave privada nunca sai do nó.
2. **Control plane.** Mantém o inventário de nós, endereços, chaves públicas, políticas e
   candidatos de endpoint. Distribui um *mapa* para cada nó e não transporta dados.
3. **Descoberta e NAT traversal.** STUN descobre o mapeamento público; o control plane troca
   esses candidatos; os nós tentam abrir um caminho UDP direto (hole punching).
4. **Data plane.** WireGuard (ou outro protocolo) cifra e autentica os pacotes. O caminho
   preferido é direto; quando o NAT/firewall impede, um relay encaminha o ciphertext.
5. **Política.** ACLs compiladas e distribuídas para os nós, aplicadas localmente. O control
   plane decide *quem pode falar com quem*; o nó decide *se aquele pacote específico passa*.

A separação entre control plane e data plane é o que permite o control plane ser barato,
público e sem privilégio, enquanto o data plane fica na máquina, perto do tráfego. O control
plane é um ponto de coordenação, não um roteador.

### 1.2 O problema concreto do Seance

O Seance é um estúdio de projetos para alunos. O aluno usa o site; o mentor e o operador
precisam alcançar o Baguette que roda em cada Mac, em `127.0.0.1:8421`, para build, print e
diagnóstico. Os Macs estão em redes domésticas diferentes, atrás de NAT, sem porta pública.

O que o wired-magic precisa entregar:

- um IP de overlay estável por Mac (por exemplo, `10.66.0.3`), para o operador abrir
  `http://10.66.0.3:8421` a partir de outro Mac da malha;
- caminho direto quando a rede permite, para latência e throughput;
- fallback confiável quando o UDP é bloqueado ou o NAT é simétrico;
- ACL para que só operadores autorizados falem com os Macs;
- uma API read-only para o Seance listar nós e IPs de overlay, sem expor isso ao navegador do
  aluno;
- operação simples: um VPS público, um binário de controle, SQLite, nenhum serviço de terceiro.

O Baguette continua em loopback. A malha não publica a porta dele na internet; ela dá ao
operador um caminho autenticado até o Mac, e o agente local decide o que escutar (o forward de
loopback é o detalhe em §2.1). O aluno nunca recebe o IP `10.66.x.x`.

### 1.3 Decisões já fechadas

| Decisão | Consequência no desenho |
| --- | --- |
| Malha independente, com API de integração | Não somos um cliente Tailscale nem um Headscale. O control plane é nosso; a integração com o Seance é read-only. |
| Linux + macOS | Data plane em userspace com boringtun + TUN; nada de módulo de kernel específico de plataforma. |
| VPS pública | Control plane, STUN e relay WSS no mesmo host público (443/TCP e 3478/UDP). A porta 41641/UDP é o socket local do agente, não um serviço do VPS. |
| Todos os Macs rodam o agente | Não há cliente de aluno; o agente é o único componente que cria TUN e fala WireGuard. |
| Alunos usam o site do Seance | Nenhum login de aluno no wired-magic; nenhum IP de overlay no navegador. |
| Sem login de aluno no nosso escopo | Identidade = token de convite + par de chaves do nó; admin = sessão do control plane. |

### 1.4 Estado do código nesta árvore

O desenho está fechado e a implementação está avançada. O que existe hoje:

| Camada | O que existe | O que falta |
| --- | --- | --- |
| `wm-proto` | Tipos de mapa, ACL, relay, admin, integração e API local; testes de ACL e framing. | Nada estrutural para o v1; é a fonte de verdade dos contratos. |
| `wm-control` | API axum completa (`enroll`, `map`, `node/*`, admin, integração), mapa, long-poll, relay WSS, STUN, SQLite, auditoria, SPA embutida (placeholder) e testes. | Métricas/observabilidade, HA, rate limit de STUN, build do painel React. |
| `wm-core` | Agente: `WgEngine` com boringtun por peer, TUN, STUN client, relay client, control client, ACL de saída/entrada com tabela de fluxos, `--expose`, timers, API local e testes de handshake/dados. | Handshake init O(1), rendezvous explícito, classificação de NAT com dois servidores, medições de produção. |
| `wm-tui` / `wm` | CLI (`host`, `serve`, `join`, `up`, `status`, `peers`, `ping`, `probe`, `resolve`, `tui`, `doctor`, `service`) e TUI Ratatui. | Empacotamento/instalação, testes de integração de ponta a ponta. |
| `deploy/`, `migrations/`, `scripts/`, `web/` | `web/dist/index.html` é um placeholder; deploy/scripts/migrações ainda não são o foco. | Empacotamento, migrações e build do painel. |

O caminho de operação já existe no binário: `wm host` sobe control plane + relay + STUN +
agente local (com tag `gateway`); `wm join <url> --token ...` matricula; `wm up` sobe só o
agente; `wm status`, `wm peers`, `wm ping`, `wm probe`, `wm resolve` e `wm tui` falam com o
socket local. Onde este documento diz “o agente faz”, leia-se “o agente faz no código atual”,
salvo quando a seção disser explicitamente “proposta” ou “a implementar”.

---

## 2. O desenho em uma página

```text
                              VPS publica
        +---------------------------------------------------------+
        | wm-control (um binario)                                 |
        |                                                         |
        |  axum /api/v1/*        HTTPS   (matricula, mapa, ACL)   |
        |  axum /relay           WSS/443 (fallback de pacotes)    |
        |  STUN                  UDP/3478 (descoberta)            |
        |  SQLite WAL            nos, tokens, ACL, auditoria      |
        +-----------+-----------------------------+---------------+
                    |                             |
          HTTPS long-poll (mapa)          WSS /relay (fallback)
                    |                             |
     +--------------+--------------+--------------+--------------+
     |                             |                             |
+----v-----+                 +-----v----+                  +-----v-----+
| Mac A    |<== WireGuard ==>| Mac B    |<== WireGuard ==> | Mac C     |
| agente   |    UDP direto   | agente   |    UDP direto   | agente    |
| tun      |                 | tun      |                  | tun       |
| 10.66.0.2|                 | 10.66.0.3|                  | 10.66.0.4 |
+----------+                 +----------+                  +-----------+

  Seance server  --GET /api/v1/integration/nodes-->  wm-control
  (server-side)                                      (read-only, token de servico)

  Aluno --> site do Seance --> Seance server
            (o navegador nunca ve 10.66.0.0/24)
```

Quatro planos, com responsabilidades separadas:

| Plano | O que carrega | Onde vive | Vê plaintext? |
| --- | --- | --- | --- |
| Controle | Matrícula, mapa, chaves públicas, ACL, candidatos, tokens | VPS | Não |
| Dados | Pacotes IP cifrados por WireGuard | Macs (direto) ou relay | Não |
| Administração | Login, nós, convites, ACL, auditoria | VPS, autenticado | Não |
| Integração | Lista read-only de nós e IPs de overlay | VPS, token de serviço | Não |

O caminho de um acesso do operador ao Baguette:

```text
operador (Mac A)                    rede                        Mac B
     |                                                             |
     | 1. abre http://10.66.0.3:8421                               |
     | 2. rota 10.66.0.0/24 -> tun                                 |
     | 3. peer B tem sessao WireGuard?                             |
     |       sim -> cifra -> UDP direto -------------------------->|
     |       nao -> handshake -> candidatos -> direto ou relay     |
     |                                                             |
     |                                        4. B decifra        |
     |                                        5. ACL de entrada   |
     |                                        6. tun / forward    |
     |                                        7. Baguette :8421   |
     |<------------------------ resposta --------------------------|
```

O ponto central: o control plane não está no caminho dos dados. Se ele cair, o caminho já
estabelecido continua; o que para é a capacidade de descobrir/alterar peers e políticas.

### 2.1 O caso do Baguette em loopback

> **Implementado:** o agente tem `--expose 8421` (listener no IP overlay → `127.0.0.1:8421`),
> sujeito à ACL. O texto abaixo mantém a análise das alternativas.
>
O TUN dá **alcance IP**, não um listener. O Baguette do Seance é iniciado em `127.0.0.1:8421`
(e o agente do Seance passa uma allowlist de Host de loopback). Um pacote para
`10.66.0.3:8421` chega ao TUN do Mac B, mas não existe socket escutando nesse IP; o kernel não
entrega o pacote ao Baguette. Para o operador abrir `http://10.66.0.3:8421`, uma das opções
abaixo precisa existir:

| Opção | Como | Prós | Contras |
| --- | --- | --- | --- |
| Baguette escuta no overlay | `--host 10.66.0.3` (ou `0.0.0.0`) | sem proxy; caminho direto | muda a exposição; precisa reiniciar o app; Host allowlist; firewall |
| Forward no agente | listener `10.66.0.3:8421` → `127.0.0.1:8421` | preserva loopback; um forward por porta; o agente já é privilegiado | cópia extra; Host header ainda pode ser validado pelo Baguette |
| Redirect de kernel (pf/iptables) | DNAT/rdr de `10.66.0.3:8421` para loopback | rápido, sem cópia userspace | específico de plataforma, root, difícil de auditar |
| Relay de aplicação no WSS | agente fala HTTP com o Baguette e repassa | não exige TUN no cliente | não é L3; um proxy por serviço; perde o desenho de malha |

**Recomendação:** o agente do wired-magic expõe uma lista **allowlisted** de portas locais (por exemplo,
`8421 -> 127.0.0.1:8421`) e faz forward TCP; nada de forward arbitrário. A ACL de ingress
limita quem alcança `10.66.0.3:8421`. Se o Baguette validar o header `Host`, o forward precisa
reescrever o Host (proxy HTTP) ou o Baguette precisa aceitar o host de overlay; isso é um ponto
de integração a validar com o app, não algo que o WireGuard resolva.

**Estado atual:** o agente ainda **não** tem esse forward. Ele cria o TUN, anuncia o IP e
roteia pacotes, mas um `wm probe 10.66.0.3 8421` só funciona se algo estiver escutando no
overlay. Até o forward existir (ou o Baguette bindar o overlay), a URL `http://10.66.0.3:8421`
não é um caminho completo.

Esse detalhe é fácil de esquecer: “ter IP de overlay” não é o mesmo que “ter o serviço
acessível”. O teste funcional deve abrir o Baguette via `10.66.0.N:8421`, não só
dar `ping` no IP. E o operador precisa de um nó na malha: o servidor do Seance, sozinho, não
alcança `10.66.0.0/24`; ele só usa a API read-only para descobrir o endereço.

---

## 3. Control plane e mapa por long-poll

### 3.1 Matrícula

O nó gera o próprio par de chaves WireGuard (X25519) e mantém a privada em disco local. Ele
envia apenas a pública no `EnrollRequest`. O control plane:

1. valida o `join_token` (hash no SQLite, TTL, usos, revogação);
2. valida/aloja um IP de overlay livre;
3. cria o nó com status `pending` ou `active`;
4. cria um `node_token` (bearer, hash no banco);
5. devolve `node_id`, `node_token`, `overlay_ip`, `overlay_cidr`, `network_name` e `control_pubkey`.

`control_pubkey` é a chave pública **Ed25519** que assina tokens de relay — não é uma chave
WireGuard e não deve ser usada para configurar um peer. O agente usa essa chave para verificar
`RelayClaims`; a chave WireGuard do control plane não existe porque o `.1` não é um peer de
dados no v1. Se um dia o `.1` virar peer, a chave WireGuard dele será um campo separado, não
este.

```text
agente                                      control                      SQLite
  |                                            |                           |
  |-- POST /api/v1/enroll {join_token, wg_pub}->|                           |
  |                                            |-- valida hash/uso ------->|
  |                                            |-- aloca 10.66.0.N ------->|
  |                                            |-- grava node + token ---->|
  |<-- {node_id, node_token, overlay_ip, ...}--|                           |
  |                                            |                           |
  |-- GET /api/v1/map?version=0&wait=25 ------>|-- SELECT nodes/ACL ------>|
  |<-- NetworkMap completo --------------------|                           |
```

Tabela de credenciais:

| Credencial | Formato | Onde é guardada | Uso | Revogação |
| --- | --- | --- | --- | --- |
| Join token | string com prefixo, hash SHA-256 no banco | mostrado uma vez | matricular nó | `revoked_at`, TTL, usos |
| Node token | bearer opaco, hash SHA-256 | no agente | mapa, endpoints, relatório | `revoked_at` ao desabilitar o nó |
| Sessão admin | cookie HttpOnly + CSRF | navegador do operador | UI/API admin | expira em 12 h, logout |
| Token de serviço | bearer opaco, hash | servidor do Seance | API de integração | `revoked_at`, escopo |
| Token de relay | Ed25519, `payload.assinatura` base64url | no mapa | abrir WSS | expira; curto por desenho |

O comentário de `consume_join_token` em `crates/wm-control/src/db.rs` é explícito: o token é
marcado como usado mesmo se a criação do nó falhar. Isso é deliberado para convites de uso
único; o operador emite outro se precisar.

### 3.2 O `NetworkMap`

O mapa é o contrato central. Ele é completo, não delta, e carrega tudo que o agente precisa
para operar sem outra chamada:

```json
{
  "version": 42,
  "protocol": 1,
  "network_name": "wired-magic",
  "overlay_cidr": "10.66.0.0/24",
  "dns_zone": "wm.internal",
  "mtu": 1280,
  "me": {
    "id": "9f1c...",
    "name": "mac-caio",
    "hostname": "mac-caio.local",
    "overlay_ip": "10.66.0.2",
    "wg_public_key": "base64...",
    "tags": ["operator"],
    "labels": {},
    "dns_name": "mac-caio.wm.internal"
  },
  "peers": [
    {
      "id": "3a7e...",
      "name": "mac-ana",
      "hostname": "mac-ana.local",
      "overlay_ip": "10.66.0.3",
      "wg_public_key": "base64...",
      "tags": ["mac"],
      "labels": {"seance": "true"},
      "endpoints": [
        {"addr": "203.0.113.7:62000", "kind": "stun"},
        {"addr": "192.168.1.20:41641", "kind": "lan"}
      ],
      "online": true,
      "last_seen": "2026-11-12T22:40:00Z"
    }
  ],
  "relay": {
    "url": "https://mesh.example.com/relay",
    "region": "default",
    "token": "base64url.payload.base64url.signature",
    "expires_at": "2026-11-12T23:40:00Z"
  },
  "stun": ["stun:mesh.example.com:3478"],
  "acl": {
    "version": 7,
    "default_allow": false,
    "rules": [
      {"action": "accept", "src": "10.66.0.2/32", "dst": "10.66.0.3/32",
       "protocol": 6, "ports": [{"start": 8421, "end": 8421}], "comment": "Baguette"}
    ]
  },
  "server_time": "2026-11-12T22:40:05Z"
}
```

O tipo está em [`crates/wm-proto/src/lib.rs`](../crates/wm-proto/src/lib.rs)
(`NetworkMap`, `PeerInfo`, `SelfInfo`, `RelayInfo`, `CompiledAcl`). Decisões embutidas:

- **Mapa completo.** Com 250 nós, o payload cresce linearmente, mas não há estado de delta,
  ordenação de patches nem risco de perder uma atualização. O agente aplica o mapa de forma
  idempotente. O custo de payload é medido em [`benchmarks.md`](benchmarks.md).
- **`me` separado de `peers`.** O nó não aparece duplicado; a UI sabe quem é “eu” sem procurar
  pelo próprio ID.
- **`relay` opcional.** Se o relay estiver desligado, o campo é `null` e o agente opera só com
  caminho direto.
- **`acl` compilada.** O agente não resolve `tag:`/`name:` em tempo de pacote; recebe CIDRs,
  protocolo e portas já resolvidos.
- **`server_time`.** Permite ao agente estimar skew de relógio para validar tokens de relay.
- **`online` é derivado.** Não é uma transição versionada; ver §3.4.

Nós `pending` e `disabled` não entram em `peers`; o admin os vê na API administrativa. O mapa
é para tráfego, não para fila de aprovação.

### 3.3 Long-poll de 25 s

O agente mantém uma requisição aberta, com o parâmetro `version` (não `since`) e um `wait`
opcional:

```text
GET /api/v1/map?version=<version>&wait=25
Authorization: Bearer <node_token>
```

O comportamento implementado em `api/node.rs` é:

- se `version != current_version`, responde imediatamente com o mapa completo;
- se `version == current_version`, espera até `wait` segundos (default 25, clamp 1..25) por uma
  mudança de versão; acorda no `watch` ou no timeout e responde o mapa completo;
- **sempre responde 200 com `NetworkMap` para nó ativo**; não há `204`, ETag ou delta no v1.

```text
agente                              handler                         store
  |                                   |                               |
  |-- GET /map?version=7&wait=25 ---->|                               |
  |                                   |-- versao atual = 7 ---------->|
  |                                   |-- subscribe(watch) ---------->|
  |                                   |                               |
  |                          (ate 25 s ou notify)                     |
  |                                   |<-- mutacao incrementa versao 8 |
  |                                   |-- monta mapa v8 ------------->|
  |<-- 200 NetworkMap v8 -------------|                               |
  |                                   |                               |
  |-- GET /map?version=8&wait=25 ---->|                               |
  |                                   |          ...                  |
```

Pseudocódigo do handler atual:

```text
fn map(version, wait):
    if version != state.current_version():
        return 200, state.build_map()
    wait = clamp(wait.unwrap_or(25), 1, 25)
    rx = state.subscribe_version()
    timeout(wait, rx.changed()).await
    return 200, state.build_map()
```

Consequência: com 250 nós, cada nó recebe um mapa completo a cada 25 s mesmo sem mudança
(≈10 respostas/s no total). Isso é simples e mantém o agente sem estado de delta, mas gasta
payload e serialização; o custo real está em [`benchmarks.md`](benchmarks.md). Se doer, a
evolução natural é `304/204` com ETag + mapa sob demanda, ou delta; não começar por delta.

Por que 25 s:

- fica abaixo de timeouts comuns de proxy/CDN/LB (30 s é um limite conservador), então a
  conexão não é cortada no meio por idle timeout;
- reduz a taxa de requisições: no pior caso, 250 nós fazem ~10 req/s no total, e não 250 req/s;
- limita a latência de detecção de uma conexão morta a um ciclo;
- o `online` de 90 s cabe três ciclos de 25 s com margem (§3.4).

A notificação é um `tokio::sync::watch` em memória, não polling no SQLite. Use `watch` (não um
`Notify` puro) para não perder a corrida entre checar a versão e dormir: o waiter relê a versão
guardada. Uma mutação que afeta o mapa incrementa a versão e acorda os waiters; cada waiter
consulta a versão e monta o mapa se precisar. O custo é O(número de waiters) por mudança,
trivial para 250.

O servidor deve configurar o proxy reverso com `proxy_read_timeout`/idle timeout acima de 30 s,
sem buffering, e permitir HTTP/1.1 ou HTTP/2. O agente reconecta com backoff exponencial (1 s, 2 s, 4 s… até 30 s). Jitter é recomendado,
mas o código atual não aplica; sem ele, o control plane que volta pode receber uma rajada de
reconexões.

### 3.4 Versionamento e presença

**Versão do mapa.** `AppState.version` é um `watch::Sender<u64>` em memória, inicializado a
partir de `settings.map_version` (ou 1) e incrementado por `bump_version()` nas mutações que
mudam o mapa: enrollment, endpoints alterados, nó aprovado/desativado, ACL publicada e relay
alterado. A versão é persistida no SQLite **no start** do control plane, não a cada mutação; um
restart pode reutilizar um número antigo. Como o mapa é sempre completo, o agente aplica o mapa
e segue; a versão serve para o long-poll saber se pode acordar, não para delta.

Regras de borda:

- **Versão igual:** espera até `wait` e devolve o mapa completo (não há `204`).
- **Versão maior:** devolve imediatamente.
- **Versão menor** (restart/restore): o agente aceita o mapa completo; a versão não é uma
  barreira de consistência porque não há delta.
- **Endpoint igual:** `update_node_endpoints` em `db.rs` devolve `changed`; só incrementa a
  versão se o conjunto serializado mudou. Isso evita um mapa novo a cada heartbeat de STUN.
- **`last_seen`:** não incrementa a versão. Se incrementasse, todo poll de 25 s viraria uma
  atualização para todos os nós, multiplicando o tráfego por n.

**Presença.** `NodeRow::online(now)` usa `ONLINE_WINDOW_SECS = 90` (`db.rs`): o nó está online
se `now - last_seen <= 90 s`, ou se o relay o tem conectado (`RelayHub::is_online`). O
`last_seen` é atualizado em qualquer request autenticado (mapa, endpoints, relatório); um loop
de presença a cada 10 s (`server.rs`) recalcula e emite `ControlEvent::Presence` quando o
estado muda. O valor de 90 s é três janelas de 25 s mais 15 s de margem.

Presença **não** é um evento de mapa. Para o agente, saber se o peer está “online” não é
necessário para rotear: ele tenta o handshake e o WireGuard responde ou não. Para o painel
administrativo, `ControlEvent::Presence { node_id, online }` pode ser enviado por SSE/WebSocket
sem tocar na versão do mapa. Essa separação evita amplificação de escrita: 250 nós batendo a
cada 25 s não devem gerar 250 mapas novos. No mapa, `online` é um snapshot do momento do build;
para a UI ao vivo, use `Presence`.

**Relógio.** `server_time` permite ao agente estimar um offset; o `verify_relay_token` atual
compara `exp` com o relógio do servidor sem janela explícita de tolerância. Adicionar ±60 s
(ou usar `server_time`) é recomendado; sem isso, um Mac com relógio adiantado pode ter o token
rejeitado.

### 3.5 Por que esse control plane é rápido e claro

- **Um request por nó por ciclo.** O long-poll não varre peers por pacote; o handler consulta o
  SQLite e monta o mapa, com `watch` em memória para acordar quando a versão muda.
- **Sem fila externa.** SQLite WAL + `watch` em memória resolvem 250 nós e uma equipe pequena.
- **Presença derivada.** O caminho quente (heartbeat) não muda a versão do mapa; o loop de
  presença só emite evento quando o estado muda.
- **Mapa completo.** O agente não precisa manter um cache diferencial; o estado é substituído.
- **Contrato único.** `wm-proto` é usado por controle, agente, CLI, TUI, web e integração.

Limites: o payload é O(n) e o rebuild é O(n + regras) **por resposta** (a cada 25 s), não só por
mudança; o SQLite tem um writer; o control plane é uma instância só (sem HA). Ver §10.3 e §13.

---

## 4. STUN e descoberta de endpoints

### 4.1 Um socket para STUN e WireGuard

O agente abre **um** socket UDP (padrão `0.0.0.0:41641`, constante `DEFAULT_UDP_PORT`) e o usa
tanto para STUN quanto para WireGuard. Isso é essencial: o mapeamento NAT que o STUN aprende é
o mesmo que o WireGuard usa. Se fossem sockets diferentes, o endpoint anunciado estaria errado.

```text
agente
  socket UDP 0.0.0.0:41641
     |
     +-- STUN Binding Request  --->  stun:mesh.example.com:3478
     |<-- XOR-MAPPED-ADDRESS    ----  203.0.113.7:62000
     |
     +-- WireGuard handshake/data --> peers (mesmo 5-tuple externo)
```

O TUN é separado; o socket UDP não carrega IP do overlay, apenas datagramas WireGuard.

### 4.2 STUN na prática

O agente envia um Binding Request com transaction ID aleatório; o servidor responde com
`XOR-MAPPED-ADDRESS` (RFC 5389/8489). A implementação atual (`wm-control/src/stun.rs` e
`wm-core/src/stun.rs`) é mínima:

- **um** servidor por vez: o agente percorre `stun_urls` e para no primeiro que responde;
- **IPv4 apenas**; resposta para IPv6 retorna vazia;
- **sem autenticação, sem rate limit e sem cookie**; é um Binding responder stateless;
- a classificação de NAT é `Cone` se qualquer STUN respondeu e `Unknown` se nenhum respondeu.
  O enum `NatType::Symmetric` existe no `wm-proto`, mas o agente atual **não** o produz.

| Observação | Classificação atual | Implicação |
| --- | --- | --- |
| um STUN respondeu | `Cone` | tenta direto; relay como plano B |
| nenhum STUN respondeu | `Unknown` | usa LAN/relay; sem conclusão sobre o NAT |
| porta muda por destino | não detectado | symmetric fica como trabalho futuro |
| IP muda | não detectado | candidatos LAN/STUN ainda podem ajudar |

Não é RFC 5780: não mede filtragem address-dependent/address-and-port-dependent nem faz
predição de porta. Para o v1, a informação “tenho um mapeamento público” já basta para tentar
o caminho direto; o relay cobre o resto. Comparar dois servidores e classificar symmetric é uma
melhoria de descoberta, não um pré-requisito.

O servidor STUN do control plane é stateless e escuta em `stun_listen` (padrão
`0.0.0.0:3478`, `DEFAULT_STUN_PORT`); é anunciado em `stun_urls`. Se o UDP 3478 estiver
bloqueado na rede do Mac, o agente pode usar outro STUN configurado; se nenhum responder, o
relay continua sendo o caminho.

### 4.3 Candidatos de endpoint

`EndpointCandidate` tem `addr` e `kind`:

| `kind` | Origem | Usado hoje? | Para que serve |
| --- | --- | --- | --- |
| `stun` | resposta STUN | sim | caminho público típico |
| `lan` | interfaces locais (`if-addrs`) | sim | peers na mesma LAN, sem sair pelo NAT |
| `previous` | último endpoint que funcionou | não | cache local; evolução |
| `manual` | configuração explícita | não | fixar endpoint; evolução |

O agente junta no máximo 16 candidatos, deduplica e ordena em `endpoint_order`:
`stun` (0), `lan` (1), `previous` (2), `manual` (3); só IPv4 entra. Ele reporta:

```text
POST /api/v1/node/endpoints
{
  "udp_port": 41641,
  "candidates": [...],
  "nat_type": "cone",
  "relay_connected": false
}
```

O control plane atualiza `nodes.endpoints`, `nat_type`, `relay_connected` e `last_seen`. O
`update_node_endpoints` devolve `changed`; só há incremento de versão quando o conjunto muda. O
loop de descoberta roda a cada **20 s** (não há gatilho imediato de mudança de interface) e o
`report` de 30 s envia o `relay_connected` real; o request de endpoints manda `false` no código
atual. O `PersistentKeepalive` de 25 s do WireGuard mantém o mapeamento aberto para peers
ativos.

### 4.4 Segurança e limites

- Candidatos são **dicas não confiáveis**. Um nó malicioso pode anunciar o IP de outra pessoa;
  o agente só envia handshakes WireGuard (148 bytes) e nada de plaintext. O handshake autentica
  o peer; um endpoint errado simplesmente não responde.
- O agente deve limitar a taxa de handshakes e de tentativas por peer (o boringtun já tem rate
  limiter de handshake; o agente adiciona limite por candidato).
- STUN não é âncora de confiança. A resposta pode ser spoofada; use transaction ID e múltiplos
  servidores, mas trate o resultado como hint.
- **Não coberto no v1:** STUN sobre TCP, UPnP/NAT-PMP, predição de porta, RFC 5780 completo,
  candidatos IPv6 no overlay (o `SocketAddr` aceita IPv6, mas o overlay é IPv4 e não há teste).

---

## 5. Hole punching e fallback por relay

### 5.1 O problema

Um NAT só deixa entrar pacote que corresponde a um mapeamento criado por tráfego de dentro
para fora. Se A quer falar com B, o pacote de A para o endpoint público de B pode ser descartado
pelo NAT de B se B nunca tiver enviado para A. O hole punching funciona quando **os dois lados**
enviam pacotes quase ao mesmo tempo: cada NAT abre uma porta para o outro, e o handshake
autenticado de WireGuard confirma o caminho.

### 5.2 O desenho

1. O control plane distribui os candidatos de A e B no mapa.
2. A precisa falar com B (pacote no TUN, `ping`, abertura de TCP 8421).
3. A inicia um handshake WireGuard; `send_network` manda a initiation para **todos** os
   candidatos de B (STUN/LAN, na ordem de `endpoint_order`). Não há escalonamento por candidato
   no código atual.
4. B precisa disparar também. É aqui que existe uma lacuna de contrato: o mapa carrega
   candidatos, mas **não** carrega um pedido explícito de “tente agora”. Sem esse sinal, B pode
   nunca iniciar, e o NAT de B descarta o pacote de A.
5. O timer do agente (`on_timer`) tenta direto a cada ~3 s enquanto houver candidatos e o
   handshake estiver velho; se não houver resposta e o relay estiver conectado/online, usa o
   relay após ~5 s. Quando está em relay e há candidatos, tenta um probe direto a cada ~30 s.
6. Se o handshake direto completar, o endpoint autenticado é fixado e o caminho vira `direct`;
   se não, o relay WSS encaminha o ciphertext.

```text
A                              control                              B
|                                 |                                 |
|-- POST /rendezvous {peer:B} --->|   (nao implementado)             |
|                                 |-- registra pedido (TTL curto) -->|
|                                 |-- mapa v43 com hint para B ----->|
|<-- mapa v43 --------------------|                                 |
|                                 |<------ GET /map?version=42 ------|
|                                 |---------- mapa v43 com hint ---->|
|                                 |                                 |
|-- WG initiation -> candidato B --------------------------------> |
|<- WG initiation <- candidato A --------------------------------- |
|   (ambos os NATs abrem; a resposta autenticada vence)             |
|                                                                   |
| se nao houver resposta na janela: troca o peer para o relay       |
```

A lacuna pode ser fechada de duas formas, ambas fora do `wm-proto` atual:

- **Rendezvous no control plane (preferida):** `POST /api/v1/rendezvous { peer }`; o control
  plane guarda um pedido com TTL (por exemplo, 10 s) e inclui `punch: [{peer, expires_at}]` no
  próximo `NetworkMap`. O mapa já é long-poll, então o hint chega em milissegundos. Custo: um
  incremento de versão por pedido; com 250 nós, pode ser agregado por janela de 1 s.
- **Rendezvous no relay:** um frame `PUNCH` (nova tag binária) entregue por WSS ao peer; o
  receptor dispara handshakes para os candidatos do emissor. Funciona com o control plane fora,
  mas exige o relay de pé e uma extensão do `RelayFrame`.

**Estado atual:** nenhuma das duas está implementada. Até lá, o agente pode tentar direto em
resposta a tráfego e a mudanças de mapa, mas não há garantia de que o outro lado dispare. Isso
é o principal item técnico em aberto para prometer hole punching (§13).

### 5.3 Seleção de caminho

O caminho preferido é direto, porque tem menos latência e não consome o relay. A seleção é por
peer, não global:

| Situação | Caminho | Ação no código atual |
| --- | --- | --- |
| Handshake recente (<120 s) | direto | manter; keepalive 25 s |
| Sem sessão, candidatos disponíveis | tentativa direta | handshake para todos os candidatos; retry a cada ~3 s |
| Direto sem resposta, relay conectado/online | relay | troca após ~5 s e envia `Data` |
| Relay ativo, candidatos ainda existem | direto (probe) | tenta handshake direto a cada ~30 s |
| Relay e direto falham | nenhum | drop/erro; reporta `last_error` |

Não há histerese sofisticada: o código atual usa os intervalos acima e troca o `path` para
`Relay` quando um pacote chega pelo relay, e para `Direct` quando um pacote direto autenticado
chega. Isso pode oscilar; medir e, se necessário, adicionar janela de sucesso/falha.

### 5.4 Relay como transporte de pacotes WireGuard

O relay não é um proxy TCP da aplicação; ele encaminha **datagramas WireGuard** dentro de frames
WSS. O agente não muda o protocolo WireGuard:

```text
A (Tunn peer B)                                     B (Tunn peer A)
   |                                                   |
   | encapsulate(pacote IP) -> ciphertext              |
   |-- Data{dst:B, ciphertext} --> relay --> Deliver{src:A, ciphertext} -->|
   |                                                   | decapsulate(ciphertext)
   |                                                   | ACL de entrada
   |                                                   | escreve no TUN
```

Consequências:

- O relay nunca vê plaintext e não precisa da chave WireGuard.
- O endpoint do peer, para o agente, é “via relay”; o `Tunn` não sabe disso. O agente alimenta
  `decapsulate` com o payload do frame `Deliver` e usa o `src` do frame para escolher a `Tunn`
  correta (não há `receiver_idx` de UDP para demuxar nesse caminho).
- O relay pode cair ou atrasar; o WireGuard continua sendo o protocolo de integridade e
  autenticação. O TCP do WSS adiciona head-of-line blocking; por isso o relay é fallback, não
  caminho de throughput.
- O MTU de 1280 mantém o pacote WireGuard pequeno o suficiente para o frame e para o caminho
  UDP direto.

### 5.5 O que esperar de cada NAT

A tabela abaixo é qualitativa; não é medição. A matriz de sucesso real está em
[`benchmarks.md`](benchmarks.md).

| NAT A | NAT B | Esperado | Estratégia |
| --- | --- | --- | --- |
| cone | cone | direto provável | hole punching |
| cone | simétrico | direto possível, mas instável | tentar; relay se falhar |
| simétrico | simétrico | relay provável | relay direto, sem insistir |
| UDP bloqueado | qualquer | relay | relay |
| CGNAT | cone | direto possível | hole punching; relay como plano B |

“Simétrico” não é sentença de morte, mas a predição de porta é cara e pouco confiável; o
desenho prefere gastar alguns segundos tentando e cair para o relay em vez de insistir.

---

## 6. WireGuard no data plane

### 6.1 Handshake

WireGuard é um protocolo L3 sobre UDP, baseado em Noise IKpsk2, com X25519, ChaCha20-Poly1305 e
BLAKE2s. O handshake é de 1 RTT e cabe em dois pacotes:

```text
iniciador                                          respondedor
   |                                                   |
   |-- msg 1: handshake initiation (148 bytes) ------->|
   |   type, sender_index, ephemeral,                  |
   |   encrypted_static, encrypted_timestamp,          |
   |   mac1, mac2                                      |
   |                                                   |
   |<-- msg 2: handshake response (92 bytes) ----------|
   |   type, sender_index, receiver_index,             |
   |   ephemeral, encrypted_nothing, mac1, mac2        |
   |                                                   |
   |-- data: type, receiver_index, counter, ciphertext |
   |<-- data ------------------------------------------|
```

Tamanhos: a initiation tem 4 + 4 + 32 + 48 + 28 + 16 + 16 = 148 bytes; a response tem
4 + 4 + 4 + 32 + 16 + 16 + 16 = 92 bytes. O `encrypted_static` carrega a chave pública do
iniciador; o `encrypted_timestamp` impede replay. `mac1` autentica o receptor; `mac2` é o
cookie sob carga. O respondedor não aloca estado para pacote não autenticado e pode responder
com cookie reply em vez de fazer X25519 quando está sob ataque.

O handshake estabelece chaves de sessão. O iniciador pode enviar dados assim que recebe a
response; o respondedor só usa a nova sessão depois de receber o primeiro pacote autenticado
(o “key confirmation”). Se o iniciador não tiver dados para enviar, manda um pacote vazio.

### 6.2 Sessões, timers e keepalive

Constantes do protocolo e da implementação (boringtun 0.7.1, `src/noise/timers.rs`):

| Timer | Valor | Efeito |
| --- | --- | --- |
| `REKEY_AFTER_TIME` | 120 s | o iniciador renegocia a sessão depois desse tempo |
| `REJECT_AFTER_TIME` | 180 s | sessão antiga é rejeitada/apagada |
| `REKEY_TIMEOUT` | 5 s | retry de handshake sem resposta |
| `KEEPALIVE_TIMEOUT` | 10 s | keepalive reativo após receber e não enviar |
| `COOKIE_EXPIRATION_TIME` | 120 s | cookie de handshake expira |
| `PersistentKeepalive` | 25 s (config) | keepalive proativo para manter NAT aberto |

A `Tunn` do boringtun mantém um ring de 8 sessões e uma fila de até 256 pacotes
(`N_SESSIONS = 8`, `MAX_QUEUE_DEPTH = 256` em `src/noise/mod.rs`). A fila segura pacotes
enquanto o handshake não completa; se enche, o boringtun descarta. O agente deve drenar a fila
com `update_timers` e `decapsulate(&[])`, e limitar tráfego por peer para não estourar memória
(ver §6.4).

`PersistentKeepalive = 25` é o valor recomendado para peers atrás de NAT: mantém o mapeamento
vivo sem inundar a rede. Para peer com endpoint público, pode ser `None`; para peer atrás de
NAT, 25 s é o padrão. O keepalive reativo de 10 s do WireGuard é diferente: ele responde a
tráfego recebido, não substitui o persistente.

### 6.3 Roaming

O WireGuard não tem “conexão” no sentido TCP. O endpoint de um peer é apenas o último endereço
de onde chegou um pacote **autenticado**. Se o Mac troca de Wi-Fi para cellular, o NAT muda, o
endereço de origem muda, mas a sessão e as chaves continuam. O próximo pacote autenticado
atualiza o endpoint; o peer responde no novo endereço.

No boringtun, o `Device` faz isso em `device/mod.rs` após um `decapsulate` bem-sucedido:
`p.set_endpoint(addr)`. No nosso agente, a regra é a mesma: **só atualizar o endpoint depois de
o pacote passar pela autenticação** (`TunnResult::WriteToTunnel*` ou handshake válido). Nunca
atualizar com base no IP de origem de um pacote não autenticado, ou um atacante fora do caminho
poderia desviar o endpoint.

Roaming também vale para o caminho relay: se o peer muda de rede, o relay continua sendo o
ponto fixo; o agente não precisa renegociar a sessão, só reconectar o WSS se o TCP cair.

### 6.4 MTU 1280

O MTU do TUN é 1280 (`DEFAULT_MTU` em `wm-proto`). A conta:

```text
pacote IP interno:            ate 1280 bytes
WireGuard transport header:   16 bytes (type 4 + receiver 4 + counter 8)
Poly1305 tag:                 16 bytes
UDP header:                    8 bytes
IP externo IPv4:              20 bytes  (IPv6: 40)
----------------------------------------------------
total no fio IPv4:           1280 + 60 = 1340 bytes
total no fio IPv6:           1280 + 80 = 1360 bytes
```

Com underlay Ethernet de 1500 bytes, 1340/1360 cabem. O valor 1280 também é o mínimo de IPv6
(RFC 8200), o que dá margem para PPPoE, CGNAT e túneis intermediários. O custo é payload útil
menor que 1420/1440; o ganho é não descobrir um black hole de MTU em produção. O TCP sobre o
TUN negocia MSS a partir dos 1280; UDP precisa respeitar o limite.

Se o underlay não suportar 1340 (raro), o agente deve reduzir o MTU ou usar o relay (TCP
segmenta; o frame WSS carrega o pacote inteiro). Não há fragmentação no WireGuard; o caminho
direto depende de DF/PMTU. Isso é medido na matriz de rede.

### 6.5 Demux O(1): `receiver_idx >> 8`

Cada peer tem sua própria `Tunn` do boringtun. Para entregar um datagrama à `Tunn` certa sem
varrer a lista de peers, usamos o índice que o próprio WireGuard carrega no pacote.

O boringtun reserva 24 bits para o índice de peer e 8 bits para o índice de sessão:

```text
receiver_idx (32 bits, little-endian no fio)
 31                                  8 7                    0
+-------------------------------------+----------------------+
|           peer_index (24 bits)      |  session (8 bits)    |
+-------------------------------------+----------------------+
                    |                            |
                    |                            +-- inc_index() incrementa
                    |                                so o byte baixo, com wrap
                    |
                    +-- Tunn::new(..., index, ...) grava index << 8
```

Código verificado no boringtun 0.7.1:

- `Tunn::new` passa `index << 8` para o handshake
  ([`noise/mod.rs#L209`](https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/mod.rs#L209));
- `inc_index` preserva os 24 bits altos e incrementa só o byte baixo
  ([`noise/handshake.rs#L466`](https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/handshake.rs#L466));
- o `Device` do boringtun demuxa por `receiver_idx >> 8`
  ([`device/mod.rs#L633`](https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/device/mod.rs#L633)).

O `WgEngine` atribui um **slot único de 24 bits por peer** (`next_slot`, 1..=0xFFFFFF) e
guarda `slot -> NodeId`; o `Tunn::new` recebe esse slot e grava `slot << 8`. O slot não é o IP
de overlay: o `/24` limita a 253 peers, mas o slot é um identificador de demux independente.
Não passe o valor já deslocado: `Tunn::new` desloca. Não use 0.

Caminho de entrada **implementado** (`wm-core/src/wireguard.rs`):

```text
UDP recv
  |
  +-- tipo 1 (handshake init)
  |     -> tenta o peer do endpoint cache (preferred)
  |     -> senao, percorre TODOS os peers e tenta decapsulate
  |        (O(n) no numero de peers; handshake init e raro)
  |
  +-- tipo 2 (handshake response), 3 (cookie), 4 (data)
        -> receiver_idx (offset 8 na response, 4 no data/cookie)
        -> slot = receiver_idx >> 8
        -> slots[slot] -> peer
        -> peer.tunn.decapsulate(Some(src_ip), pacote, dst)   (O(1))
```

O caminho de **dados** é O(1), como planejado. O handshake init é a exceção: como o
`sender_index` pertence ao iniciador, o engine atual não tem como mapeá-lo a um peer; ele tenta
o endpoint cache e depois todos os peers. Isso é aceitável para handshakes raros, mas custa
O(n) tentativas de decapsulate (e possível X25519) por init, o que importa sob flood ou com 250
peers.

**Otimização recomendada (não implementada):** usar
`parse_handshake_anon(static_private, static_public, packet)` para extrair a chave estática do
iniciador, fazer lookup O(1) em `HashMap<[u8;32], NodeId>` e só então chamar `decapsulate`.
Antes disso, um `RateLimiter` compartilhado (MAC1/cookie) deve barrar flood, porque o
`parse_handshake_anon` faz X25519 antes do lookup. O `Device` do boringtun faz exatamente essa
ordem: `verify_packet` → `parse_handshake_anon` → `peers` (hash pela chave pública); para
data/response ele usa `peers_by_idx`.

Enquanto a otimização não entra, o custo de handshake init é O(n) e deve ser medido no cenário
de 250 peers; um atacante que consiga enviar inits válidos para o MAC1 pode forçar trabalho.

Por que não usar uma única `Tunn` para todos os peers: a `Tunn` do boringtun é por peer (tem
handshake, sessões e fila de um peer). O `Device` do boringtun existe para multi-peer, mas ele
é dono do socket UDP e do TUN/UAPI, o que conflita com o nosso relay, STUN e ACL. A escolha é
uma `Tunn` por peer + um demux nosso; o custo é implementar timers e roteamento, o ganho é
controle total do caminho.

### 6.6 Timers, filas e concorrência

O agente tem tarefas de I/O e **um** loop de dados:

1. **UDP recv:** lê datagramas, separa STUN e manda `Cmd::Udp` para o `data_loop`.
2. **TUN recv:** lê pacotes IP e manda `Cmd::Tun` para o `data_loop`.
3. **Timer:** a cada **100 ms** manda `Cmd::Timer`; o `data_loop` chama `update_timers` em cada
   `Tunn`, drena `WriteToNetwork` e envia.
4. **`data_loop`:** é o único dono do `WgEngine`, do socket, do TUN e da ACL; recebe os comandos
   por um `mpsc` de 8.192 e processa em série. Não há `DashMap` nem lock por peer; a
   serialização é o “lock”. Isso simplifica o estado, mas limita o caminho a um núcleo; medir
   e, se necessário, shardar por peer ou usar `SO_REUSEPORT`.

O timer de 100 ms com 250 peers dá ~2.500 chamadas de `update_timers`/s, além do tráfego; é
barato, mas medir. O `WgEngine` também aloca buffers por pacote (`vec![0u8; 2048]` em
`decapsulate`, `to_vec` nas saídas); isso é um custo de CPU/alocação a medir e otimizar.

Memória: cada `Tunn` tem 8 sessões e uma fila de até 256 pacotes. Pior caso teórico com 250
peers e MTU 1280: 250 × 256 × 1280 ≈ 78 MiB (81,9 MB) só de payload das filas, se todas
encherem ao mesmo tempo. Na prática as filas ficam vazias; ainda assim, o limite existe e deve
ser medido. Não há knob de `MAX_QUEUE_DEPTH` no boringtun 0.7.1.

### 6.7 Anti-spoof e “cryptokey routing”

O WireGuard de kernel usa `AllowedIPs` para amarrar chave pública ↔ prefixo. Nós não usamos
isso no kernel; a regra equivalente fica no agente:

- **ingress (implementado):** depois de decifrar, `src == peer.overlay_ip` e
  `dst == me.overlay_ip`; senão, drop;
- **egress (parcial):** o IP de destino precisa pertencer a um peer do mapa; a ACL de saída usa
  `me.overlay_ip` como origem, mas o código atual **não** compara `meta.src` com o IP local
  antes de cifrar. Um pacote local com src forjado chega ao peer, mas o ingress do peer o
  descarta porque `src != peer.overlay_ip`; a defesa final existe, a checagem na origem não;
- o agente não faz forwarding entre peers; `net.ipv4.ip_forward` fica desligado.

Essas checagens são baratas (comparação de `Ipv4Addr`) e impedem que um peer injete pacotes
com o IP de outro. A ACL (§8) é uma camada adicional, não um substituto. Adicionar a checagem
de `meta.src` no egress é uma correção pequena e recomendada.

---

## 7. Relay WSS

### 7.1 Por que WSS na 443, no mesmo host

O relay existe para o caso em que o caminho UDP direto não funciona: NAT simétrico, firewall
corporativo que bloqueia UDP, rede que só deixa sair 443/TCP. WSS na 443:

- atravessa os mesmos proxies/firewalls que o HTTPS do control plane;
- reutiliza o certificado, o DNS e a porta já abertos;
- não exige um segundo serviço, uma segunda conta ou uma segunda hospedagem;
- tem framing binário e conexão persistente, adequados a datagramas;
- é TLS, então o token e o ciphertext não ficam em claro.

O preço é TCP: head-of-line blocking, retransmissão e latência sob perda. Por isso o relay é
**fallback**, não o caminho principal. Se o tráfego relayado crescer, a evolução é multi-região
ou QUIC; não no v1.

### 7.2 Frames

Um frame é uma mensagem binária WebSocket. O primeiro byte é a tag; os dados seguem em binário,
sem JSON e sem base64 por pacote.

| Tag | Nome | Direção | Conteúdo |
| --- | --- | --- | --- |
| `0x01` | `Data` | agente → relay | `dst` UUID (16) + ciphertext WireGuard |
| `0x02` | `Deliver` | relay → agente | `src` UUID (16) + ciphertext |
| `0x03` | `Ping` | ambos | vazio |
| `0x04` | `Pong` | ambos | vazio |
| `0x05` | `Presence` | relay → agente | `node` UUID (16) + online (1) |
| `0x06` | `Error` | relay → agente | mensagem UTF-8 |

`RELAY_MAX_PAYLOAD = 64 * 1024` (`wm-proto`). O pacote WireGuard no MTU 1280 tem ~1.3 KB, então
64 KiB é folga para framing/batch futuro e um limite de memória. O `encode` rejeita payload
maior; o `decode` **não** rejeita, então o servidor precisa impor o limite no read do WebSocket
antes de decodificar. Um frame `Data` sem os 16 bytes de UUID é inválido.

```text
A                         relay (DashMap<NodeId, Sender>)                  B
|-- Data{dst:B, wg} ------------------->|                                  |
|                                       |-- lookup B -> Sender ----------->|
|                                       |-- Deliver{src:A, wg} ----------->|
|                                       |                                  |
|<-- Deliver{src:B, wg} ----------------|<-- Data{dst:A, wg} --------------|
```

O relay não decodifica o payload; ele só vê UUIDs, tamanhos e horários. Isso é suficiente para
rotear e insuficiente para ler o tráfego. Se o relay mentir sobre o `src` de um `Deliver`, o
pacote é entregue à `Tunn` errada e falha na autenticação do WireGuard; a confiança final é do
protocolo, não do relay.

### 7.3 Token assinado

O control plane assina um `RelayClaims` com Ed25519:

```json
{"sub": "3a7e...", "aud": "https://mesh.example.com/relay",
 "iat": 1763000000, "exp": 1763003600}
```

O token é `base64url(payload).base64url(assinatura)`, não é JWT (não tem header). O TTL é
limitado entre 60 s e 24 h por `sign_relay_token` (`auth.rs`); o código atual usa **1 h**
(`relay_info_for`). O desenho recomenda **15 min** como endurecimento, mas isso ainda não foi
aplicado. O agente recebe o token no `RelayInfo` do mapa e o envia no header
`Authorization: Bearer <token>` no upgrade do WSS (não na query string, para não vazar em logs
de proxy). O `RelayInfo.url` já é a URL completa, incluindo `/relay`.

O relay verifica:

1. assinatura com a chave pública do control plane;
2. `exp` (o `verify_relay_token` usa o relógio do servidor; tolerância de skew é desejável,
   mas o código atual não aplica uma janela explícita);
3. `aud` exatamente igual à URL pública do relay (`https://host/relay`);
4. `sub` existe, está `active` e não está `disabled`;
5. se já há conexão para `sub`, o registro é substituído no `RelayHub`; a conexão antiga tende
   a cair quando o sender é descartado.

O token não tem nonce nem lista de revogação. **O revoke atual não fecha a conexão WSS ativa:**
o token é checado apenas no upgrade; um nó desabilitado pode continuar usando o relay até a
conexão cair ou o token expirar (1 h). Fechar a conexão no `revoke_node` via `RelayHub` é uma
correção necessária. Se o relay for separado no futuro, ele precisa de introspecção
(`/internal/relay/authorize`) ou de uma lista de revogação; até lá, o TTL é o limite.

### 7.4 Ciclo de vida e backpressure

- O agente conecta no `/relay` com TLS, valida o certificado, manda o Bearer, espera o upgrade.
- Registra presença; o relay informa quem já está online e faz broadcast da entrada/saída.
- O servidor manda `Ping` a cada **25 s**; o cliente responde `Pong`. O cliente manda `Ping` a
  cada **20 s** e o servidor responde `Pong`. O cliente atual **não** fecha por Pong ausente;
  fecha em erro/close. Timeout explícito de Pong é melhoria.
- Reconexão com backoff exponencial de 1 s a 30 s (sem jitter no código atual).
- Cada conexão do relay tem um canal de 512 frames; o cliente tem um canal de saída de 2.048.
  `try_send` falha quando enche e o frame é descartado; não há bloqueio do loop de leitura. O
  agente reenvia/retransmite no nível WireGuard/TCP.
- O token é renovado a cada mapa (25 s) porque cada `build_map` assina um novo `RelayInfo`; a
  conexão WSS existente não é reaberta só porque o token mudou. Em reconexão, o cliente usa o
  `RelayInfo` mais recente.
- O agente só envia `Data` para peers que estão no mapa e com `relay_connected`; não confia no
  `dst` para decidir política (a ACL é aplicada antes, no nó).
- O relay não persiste nada; reiniciou, todos reconectam. O estado de presença é do control
  plane/mapa, não do relay.

### 7.5 Por que não QUIC, TCP bruto ou TURN no v1

| Alternativa | Por que não no v1 |
| --- | --- |
| QUIC | UDP 443 é justamente o que muitas redes bloqueiam; o relay precisa do caminho mais compatível. QUIC adiciona complexidade de transporte e multiplexação sem resolver o caso “só 443/TCP”. Revisitar se o relay virar caminho de throughput. |
| TCP bruto (sem TLS) | Não atravessa proxy corporativo, não tem autenticação padrão, exige framing próprio e não melhora nada em relação a WSS. |
| TURN/Coturn | Mais um serviço, alocações, autenticação e keepalive; o TURN sobre TCP sofre do mesmo HOL; não precisamos de alocação porque já temos um control plane e um relay simples. |
| DERP | Protocolo próprio da Tailscale (código aberto, mas específico), implementação Go, multi-região; funciona, mas é uma dependência grande para um caso de 250 nós. Copiamos a ideia (relay de ciphertext na 443), não o protocolo. |
| HTTP POST por frame | Overhead de request/response e HOL; não é bidirecional persistente. |
| WebRTC data channel | STUN/ICE/DTLS/SCTP demais para um relay de pacotes; o agente já tem WireGuard. |

---

## 8. ACL distribuída

### 8.1 Política

A política é um YAML com regras direcionais. Seletores:

- `*` — todos;
- `name:mac-ana` — nome exato do nó;
- `tag:operator` — nós com a tag;
- CIDR — `10.66.0.0/24`, `10.66.0.3/32`.

Exemplo:

```yaml
version: 7
rules:
  - action: accept
    src: ["tag:operator"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["8421"]
    comment: "Baguette"
  - action: accept
    src: ["tag:operator"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["22"]
    comment: "SSH de diagnostico"
  - action: deny
    src: ["*"]
    dst: ["tag:mac"]
    protocol: "*"
    ports: ["*"]
    comment: "fecha o resto"
```

O tipo `AclPolicy`/`AclRule` está em `wm-proto`; o `AclAction` é `accept` ou `deny`. O
`protocol` aceita `tcp`, `udp`, `icmp` ou `*`; `ports` aceita `*`, `22`, `8000-8100`.

### 8.2 Compilação

O control plane resolve os seletores para os IPs de overlay **no momento de salvar** e guarda
o YAML + o compilado (`acl_policies` em `db.rs`). Regras com múltiplos nós são expandidas para
um `CompiledRule` por par src/dst (`/32` para nó, CIDR para `*`/CIDR), preservando a ordem; não
há agregação de CIDRs no compilador atual. O limite é 8.192 regras compiladas. O resultado é
`CompiledAcl`:

```json
{
  "version": 7,
  "default_allow": false,
  "rules": [
    {"action": "accept", "src": "10.66.0.2/32", "dst": "10.66.0.3/32",
     "protocol": 6, "ports": [{"start": 8421, "end": 8421}], "comment": "Baguette"}
  ]
}
```

Semântica:

- **first-match wins**: a primeira regra que casa decide; não há “merge” de regras;
- **direcional**: aceitar A→B não aceita B→A; a tabela de fluxos cobre a resposta de
  conexões estabelecidas (mas não é um conntrack completo)
  inversa ou de default allow (ver §8.3);
- **default**: o compilador atual define `default_allow = rules.is_empty()`. Sem nenhuma regra
  compilada, é allow (bootstrap). Com pelo menos uma regra, é deny-by-default. Se todos os
  seletores de uma regra não resolverem (nome/tag inexistente), ela vira inerte e pode deixar
  `rules` vazio — nesse caso o default volta a allow; os warnings de compilação precisam ser
  tratados como erro de operação. Para liberar tudo, escreva `accept * -> *` explícito.

Recompilar em mudança de nó/tag: se uma regra usa `tag:` ou `name:`, criar/renomear/retaggar
um nó pode mudar o conjunto de CIDRs. O `put_acl` compila no save; **não há recompilação
automática quando um nó muda de tag/nome**, então uma regra `tag:` pode ficar stale até a
próxima publicação. Recompilar no `update_node`/`approve` (ou no build do mapa) é uma correção
necessária; o `validate_acl` mostra os warnings.

### 8.3 Aplicação nos dois lados

A mesma `CompiledAcl` é aplicada na origem e no destino. O agente mantém uma **tabela de
fluxos** (5 min) para TCP/UDP e eco ICMP; o texto abaixo descreve o modelo original sem ela:

```text
EGRESS (origem)                                   INGRESS (destino)
pacote do TUN                                     pacote decifrado
  |                                                 |
  +-- dst pertence a um peer? nao -> drop           +-- src == peer.IP? nao -> drop
  +-- acl.is_allowed(me, dst, proto, port)?         +-- dst == meu IP? nao -> drop
  |     nao -> drop                                 +-- acl.is_allowed(peer, me, proto, port)?
  |     sim -> encapsula e envia                          nao -> drop
  |                                                       sim -> escreve no TUN
  +-- (o codigo atual NAO verifica meta.src == me)
```

Por que os dois lados:

- **origem**: evita gastar CPU/rede cifrando pacote que o destino vai negar; é o ponto onde o
  emissor tem contexto (usuário, processo, intenção);
- **destino**: é a última linha de defesa; um nó comprometido ou uma versão antiga da ACL não
  consegue injetar tráfego que o destino não aceita;
- **anti-spoof**: no ingress, `src == peer.overlay_ip` impede que um peer se passe por outro;
  no egress, a checagem explícita de `meta.src` ainda falta (o peer rejeita no ingress);
- **defesa em profundidade**: as duas checagens usam a mesma política, mas falham de forma
  independente.

**Consequência do stateless.** O matcher é first-match e não guarda fluxo. Uma regra
`accept operator -> mac:8421` libera a requisição, mas a resposta TCP do Mac para o operador
tem outra 5-tuple e será negada se a política for deny-by-default. Hoje isso exige uma regra
espelhada (por exemplo, `accept mac -> operator` em portas efêmeras) ou um default allow; para
o Baguette, o caminho mais simples é uma regra de resposta ampla, o que enfraquece a política.

**Conntrack (proposta, não implementada).** Para não exigir regra espelhada:

- chave: 5-tuple normalizada;
- TCP: cria no SYN aceito; mantém até FIN/RST ou timeout (proposta: 300 s);
- UDP: 60 s; ICMP echo pelo identificador;
- limite: 4.096 fluxos por agente, LRU; se encher, volta ao matcher stateless;
- o primeiro pacote de cada fluxo passa pela ACL nos dois lados; só a resposta de fluxo
  estabelecido é liberada.

Sem conntrack, a ACL dos dois lados funciona, mas a experiência de operação fica pior. A
decisão de implementar conntrack é o principal item de ACL em aberto.

**Falha do control plane.** O agente mantém a última ACL válida e continua aplicando. Não há
fail-closed por padrão, porque derrubar todo o tráfego quando o control plane cai é pior para
a operação. O modo fail-closed pode existir como opção para redes de alta segurança; ficaria
explícito no config e no mapa. O custo é que uma revogação feita durante a queda não chega até
o nó; ver §10.1.

**Relay e ACL.** O relay não aplica ACL; ele só encaminha. A ACL é aplicada no emissor e no
receptor. Um relay malicioso pode descartar/atrasar, mas não ler nem forjar o ciphertext.

### 8.4 Simulação e auditoria

`SimulateRequest`/`SimulateResponse` em `wm-proto` permitem ao admin testar
`src`, `dst`, `protocol`, `port` e ver `allowed`, `rule_index` e `reason`. As rotas existem:
`PUT /api/v1/admin/acl` publica, `POST /api/v1/admin/acl/validate` compila e devolve warnings,
`POST /api/v1/admin/acl/simulate` usa a mesma `CompiledAcl` que os agentes recebem. Toda
publicação de ACL gera `audit_events` com ator, versão e resumo; o agente registra drops em
log de debug, sem contador dedicado no código atual.

---

## 9. Endereçamento, DNS e MTU

### 9.1 `10.66.0.0/24`

| Endereço | Uso |
| --- | --- |
| `10.66.0.0` | rede |
| `10.66.0.1` | identidade do control plane/gateway reservado; **não é um peer WireGuard no v1** |
| `10.66.0.2`–`10.66.0.254` | nós (253 endereços) |
| `10.66.0.255` | broadcast |

A alocação em `create_node` (`db.rs`) percorre `net.hosts()`, pula o `.1` e o broadcast, e pega
o primeiro livre; o índice é o menor IP disponível, não aleatório. O IP é estável: fica no
SQLite e só muda se o nó for removido e recriado.

Por que `10.66.0.0/24`:

- **Compatibilidade com o Seance.** A pesquisa R11/D022 do Seance já usa `10.66.0.0/24` para
  o control plane de malha e mostra `10.66.0.5` nas configs. O wired-magic mantém o mesmo
  espaço, então a integração e a documentação não precisam traduzir faixas.
- **Evita `100.64.0.0/10`.** Tailscale e NetBird usam CGNAT (`100.64.0.0/10`) por padrão; se um
  Mac já tem Tailscale instalado (o host de desenvolvimento tem), uma segunda VPN na mesma
  faixa cria ambiguidade de rota. `10.66.0.0/24` é RFC 1918 e não colide com o tailnet.
- **É privado e roteável só no overlay.** Não é anunciado na internet; o TUN recebe uma rota
  `10.66.0.0/24` e nada de default route.

Riscos: a LAN do escritório pode usar `10.66.0.0/24`. Nesse caso, o operador troca
`overlay_cidr` na config (o validador aceita prefixo de rede ≤ `/24`; ver `config.rs`) ou ajusta a LAN.
O endereço `.1` é reservado para uma futura presença do control plane (DNS/metrics); ele não
é um gateway de tráfego e não deve ser usado como próximo salto.

### 9.2 DNS

`DEFAULT_DNS_ZONE = "wm.internal"` e `SelfInfo.dns_name` existem para a UI e para um futuro
resolver. **No v1 não há MagicDNS**: o agente não roda servidor DNS e o mapa não distribui
nameserver. O operador usa IP de overlay ou adiciona uma entrada local. Um resolver por nó
(ou um DNS no `.1`) é trabalho futuro; não é pré-requisito para o Baguette.

### 9.3 MTU

1280, pelos motivos de §6.4. O TUN recebe `mtu 1280`; a rota `10.66.0.0/24` aponta para ele.
Não há `0.0.0.0/0`; não há exit node.

---

## 10. Revogação e tolerância a falhas

### 10.1 Revogação em camadas

| Camada | Ação | Efeito | Latência |
| --- | --- | --- | --- |
| Convite | `revoke_join_token` | impede novos nós com aquele token | imediata |
| Nó | `set_node_status(disabled)` | status `disabled`, `node_tokens.revoked_at`, nó sai do mapa | imediata no banco; mapa no próximo ciclo |
| Relay | token expira ou conexão cai | nó não usa mais o relay | TTL de **1 h** ou queda da conexão |
| Pares | remoção da chave pública do mapa | peers atualizados removem a `Tunn`; pacotes do nó revogado deixam de casar | próximo mapa (≤25 s) |
| Auditoria | `audit_events` | registro de quem revogou e quando | imediata |

```text
t0  admin desabilita o no
      -> DB: status=disabled, tokens revogados, versao do mapa++
      -> relay: NAO fecha a conexao ativa no codigo atual; ela segue ate cair/expirar
      -> agentes: long-poll acorda ou retorna no proximo ciclo; mapa sem o no
t0+25s  limite para um agente que estava dormindo/atrasado aplicar
t0+1h   limite do token de relay (TTL atual); fechar no revoke e a correcao recomendada
```

**Limite importante.** A chave WireGuard é estática e vive no nó. Um nó revogado que ficou
offline e voltou com o mapa antigo ainda pode tentar handshake com peers que também estão com
o mapa antigo. Depois que um peer recebe o mapa novo, ele não tem mais a chave pública do
revogado e rejeita o handshake. A janela é a latência de propagação (o long-poll retorna no próximo ciclo, no pior caso 25 s)
mais o tempo em que o peer ficou offline. Não há como revogar
criptograficamente um peer em uma rede WireGuard estática sem rotação de chaves.

Mitigações:

- TTL curto do token de relay;
- desabilitar o nó remove tokens e mapa;
- rotacionar o join token após incidente;
- para comprometimento de chave, remover o nó e recriá-lo; uma rotação de chave de toda a malha
  (re-enroll) é trabalho futuro e cara;
- logs de auditoria e alerta de handshake de chave desconhecida.

**Limite do engine atual:** `WgEngine::apply_peers` atualiza `peer.public_key` quando o mapa
muda, mas **não recria a `Tunn`** se a chave pública de um peer existente mudar. Na prática, a
rotação de chave de um peer exige removê-lo/readicioná-lo no mapa (ou reiniciar o agente). Isso
reforça a recomendação de tratar comprometimento de chave como remoção + recriação do nó.

### 10.2 Matriz de falhas

| Falha | Impacto | Comportamento esperado | Recuperação |
| --- | --- | --- | --- |
| Control plane cai | Sem novos nós, sem mudanças de ACL/mapa; dados diretos continuam; se o relay é o mesmo processo, o fallback também cai | Agente mantém último mapa/ACL; sessões novas com peers conhecidos e endpoints cacheados podem ser refeitas; long-poll reconecta com backoff | control volta; agente refaz `map?version=last` |
| Relay cai | Pares relayados perdem conectividade; diretos não | Agente tenta direto de novo; relay reconecta | relay volta; reconexão |
| STUN cai | Sem novos candidatos públicos | Usa LAN; relay como plano B (não há cache `previous` no código atual) | STUN volta |
| Peer offline | Sem tráfego para ele | Handshake retry; `last_seen`/handshake stats; presença 90 s | peer volta; roaming/handshake |
| Mudança de rede | Endpoint antigo inválido | Não há detector de interface; o loop STUN/report de 20–30 s atualiza os candidatos; roaming no peer | próximo handshake |
| Agente reinicia | Sessões perdidas | Recarrega chave/token; `map?version=0`; recria Tunns | handshake |
| Control reinicia | Long-polls caem | Agentes reconectam com backoff; versão inicial persistida no SQLite, mapa completo | automático |
| SQLite restaura backup | Versão pode regredir | Agente aceita mapa completo mesmo com versão menor | automático |
| Relógio skew | Token de relay rejeitado | `server_time` + tolerância; NTP no VPS | corrigir relógio |
| MTU black hole | Pacotes grandes somem | 1280; medir; reduzir ou relay | ajuste |
| Relay sobrecarregado | HOL/drop | Backpressure, drop de dados, fecha conexão lenta | capacity/multi-região |
| Control sobrecarregado | Latência de mapa | 250 nós, SQLite; rate limit; sem HA | vertical/HA futuro |

### 10.3 Limites de disponibilidade

- **Uma instância de control plane.** Não há HA no v1. Se o VPS cai, o data plane direto
  continua para sessões estabelecidas; novas descobertas e mudanças de ACL param.
- **Um relay.** Todos os pares relayados dependem dele. Não há multi-região nem failover
  automático. O relay é o SPOF do caminho de fallback.
- **SQLite.** WAL, `busy_timeout` de 5 s, 8 conexões, um writer. Backup precisa incluir o WAL
  ou usar `VACUUM INTO`/checkpoint; um `cp` do `.db` sem checkpoint pode perder transações.
- **Sem replicação.** Um restore de backup pode perder nós/ACL recentes; o mapa é completo, mas
  o estado autoritativo é o banco.

Para o porte do Seance (uma operadora, dezenas de Macs, alvo de 250 nós), isso é um trade-off
consciente: menos peças, menos custo, menos superfície. Se o requisito virar 99,9%, a conversa
muda para Postgres/HA e relay multi-região; não é o v1.

---

## 11. Por que o desenho é rápido, claro e funcional

### 11.1 Rápido

| Mecanismo | Efeito | Custo/limite |
| --- | --- | --- |
| Caminho direto por padrão | evita relay e HOL | depende de NAT/firewall |
| Demux O(1) no data path por `receiver_idx >> 8` | uma busca por pacote, sem varrer peers | handshake init ainda é O(n) no engine atual |
| Uma `Tunn` por peer | estado de sessão isolado; sem lock global | um único `data_loop` serializa o processamento |
| ACL compilada | sem resolver nome/tag por pacote | recompilar em mudança de membro |
| Frames binários no relay | sem JSON/base64 por pacote | relay é fallback, TCP HOL |
| Long-poll de 25 s | convergência rápida quando a versão muda | o código devolve mapa completo a cada ciclo |
| Presença derivada | heartbeat não muda a versão do mapa | presença pode ficar stale |
| SQLite WAL | sem serviço externo; leitura concorrente | um writer; sem HA |
| MTU 1280 | evita fragmentação/black hole | menos payload por pacote |

O que pode deixar o caminho lento e precisa ser medido: cópias userspace/TUN, alocação de buffer
por pacote no `WgEngine`, custo de ChaCha20-Poly1305 no CPU do Mac, fila de 256 pacotes por
peer, rebuild/serialização do mapa **a cada 25 s** (não só em mudança), handshake init O(n) e
HOL do relay. Nenhum número aqui é medido; a metodologia está em
[`benchmarks.md`](benchmarks.md).

### 11.2 Claro

- **Um contrato de mapa.** `NetworkMap` é a única entrada de estado do agente; não há uma
  segunda API para “descobrir peer” ou “pegar ACL”.
- **Um framing de relay.** `RelayFrame` é um enum com tags de 1 byte; o relay não tem parser
  de WireGuard.
- **Uma ACL.** `AclPolicy` (autoral) → `CompiledAcl` (fio) → `is_allowed` (nó). O admin simula
  com a mesma função.
- **Um banco.** SQLite com schema explícito; sem Redis, NATS, etcd ou Postgres no v1.
- **Um binário de controle.** axum + SQLite + relay + STUN no mesmo processo; um deploy, um
  certificado, um firewall.
- **Estados explícitos.** `NodeStatus`, `NatType`, `PathKind`, `EndpointKind`, `AclAction`;
  erros tipados (`ApiError`, `ProtoError`).
- **Auditoria.** Ações administrativas e publicações de ACL deixam `audit_events`.

### 11.3 Funcional

- O data plane, o control client, o STUN client, o relay client e a API local existem no
  `wm-core`; o control plane, o relay, o STUN e a API admin/integração existem no `wm-control`.
- Funciona atrás de NAT doméstico/CGNAT porque tenta direto e cai para relay.
- Funciona em macOS e Linux porque o data plane é userspace e o TUN é a única dependência de
  sistema (com root/CAP_NET_ADMIN).
- Não exige cliente de aluno, login de aluno ou navegador especial.
- Integra com o Seance por uma API read-only, sem acoplar o banco.
- Sobrevive à queda do control plane para o tráfego já estabelecido; novas sessões com peers
  conhecidos e endpoints cacheados também podem ser refeitas.
- É operável por uma pessoa: `wm host`, `wm join`, `wm status`, `wm peers`, `wm ping`, `wm tui`.

**O que ainda não está completo:** o sinal explícito de rendezvous, o handshake init O(1), a
classificação de NAT com dois servidores e a
validação de ponta a ponta com 250 nós. Ver §1.4 e §13.

### 11.4 O que pode dar errado

- NAT simétrico + UDP bloqueado: o relay vira o caminho normal, e o throughput/latência do
  WSS/TCP passam a importar.
- Relay único: um pico ou uma queda derruba o fallback.
- Revogação de nó offline: janela de mapa stale (§10.1); o relay ativo só cai no TTL de 1 h.
- Fila por peer: pico de tráfego com handshake pendente pode consumir memória.
- Mapa completo a cada 25 s: 250 nós geram ~10 mapas/s e rebuild O(n); medir.
- Handshake init O(n) e alocação por pacote no engine atual; medir sob carga/flood.
- ACL: a tabela de fluxos cobre respostas TCP/UDP/ICMP estabelecidas; não é conntrack completo.
- Baguette em loopback: use `--expose 8421`; sem isso, `10.66.0.N:8421` não abre.
- Privilégios: criar TUN no macOS/Linux exige root/CAP_NET_ADMIN; o instalador/launchd é uma
  decisão de deploy, não do control plane.
- Relógio: token de relay depende de `exp`; skew grande quebra a conexão.

---

## 12. Comparação com Tailscale/Headscale, NetBird e Nebula

### 12.1 Visão geral

| Projeto | Data plane | Control plane | NAT traversal | Relay | ACL | Endereçamento | Pegada |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Tailscale** | WireGuard | coordination service (hospedado) | STUN + hole punching | DERP (TLS, protocolo próprio) | policy compilada, aplicada no nó (ingress), deny-by-default | `100.64.0.0/10` | SaaS ou Headscale + DERP |
| **Headscale** | WireGuard (cliente Tailscale) | self-hosted, single tailnet | igual ao cliente Tailscale | DERP embutido ou externo | ACLs/grants do Tailscale | `100.64.0.0/10` | control + DERP + clientes Tailscale |
| **NetBird** | WireGuard | management + signal | Pion ICE/STUN | Coturn/TURN; relay WS mais novo | políticas por grupo aplicadas no cliente | `100.64.0.0/10` (bloco por conta) | 4 serviços + dashboard/IDP |
| **Nebula** | Noise (não WireGuard) | lighthouses + PKI | UDP hole punching via lighthouse | relay configurável | firewall por grupos/certificados | configurável | lighthouse + CA |
| **wired-magic** | WireGuard (boringtun) | axum/SQLite | STUN + hole punching | WSS/443 | ACL first-match, origem + destino | `10.66.0.0/24` | 1 binário de controle + agente + SQLite |

### 12.2 O que foi adotado

**De Tailscale/Headscale:**

- separação control plane/data plane, com o control plane fora do caminho dos dados;
- mapa completo e versionado, entregue por long-poll (o cliente Tailscale usa um `/map` com
  long poll; ver `direct.go`);
- STUN + candidatos + preferência por caminho direto. A Tailscale começa a conexão pelo DERP e
  tenta upgrade para direto; nós já temos os candidatos no mapa e tentamos direto primeiro,
  usando o relay como fallback. O objetivo é o mesmo; a ordem é diferente;
- relay de ciphertext na 443 como fallback, sem capacidade de decriptar;
- IP de overlay estável por nó;
- ACL compilada e distribuída, deny-by-default quando há regras;
- `PersistentKeepalive = 25` e roaming por endpoint autenticado;
- tolerância a control plane offline: último mapa/ACL continua valendo (a documentação da
  Tailscale descreve o mesmo comportamento para conexões já estabelecidas).

**De NetBird:**

- WireGuard como data plane e um management que distribui chaves/config;
- relay como “plano B” explícito, não como caminho principal;
- ideia de aplicar ACL no cliente a partir da política do control plane.

**De Nebula:**

- lighthouses como conceito de rendezvous: um ponto conhecido ajuda os pares a se encontrarem;
  no wired-magic, o control plane + STUN fazem esse papel;
- firewall por grupos (nós tags), que inspirou a ACL por tag.

**De WireGuard puro:**

- o protocolo e a criptografia; não reinventamos o data plane.

### 12.3 O que foi rejeitado e por quê

| Rejeitado | Por quê |
| --- | --- |
| `100.64.0.0/10` do Tailscale/NetBird | colide com um Tailscale já instalado no Mac; o Seance já usa `10.66.0.0/24`; a integração fica mais simples. |
| DERP como protocolo | é um protocolo próprio da Tailscale (código aberto, mas específico), em Go e multi-região; copiamos a ideia de relay de ciphertext na 443, não o protocolo. |
| Tailscale/Headscale como dependência | acrescenta um terceiro serviço, acopla ao cliente Tailscale e limita o controle sobre ACL, metadados e integração; a decisão do dono foi malha independente. |
| `tsnet` embutido | Go, stack inteira, menos controle sobre UDP/relay/ACL; não é o nosso runtime. |
| NetBird completo | management + signal + relay + dashboard + IDP é muita peça para 250 nós e uma operadora; o ICE/TURN do NetBird é mais geral do que precisamos. |
| Nebula | Noise/PKI próprio, sem WireGuard; bom projeto, mas exigiria trocar o data plane e a identidade. |
| WireGuard puro + `wg-quick` | não resolve descoberta, NAT traversal nem relay sozinho; o servidor teria que ser um hub UDP público, e Macs atrás de CGNAT continuariam sem caminho. |
| Relay como caminho primário | adiciona latência/HOL e transforma o VPS em gargalo; o direto é melhor quando possível. |
| QUIC no relay v1 | UDP 443 é bloqueado nas redes que mais precisam do fallback; WSS/TCP é mais compatível. |
| ACL só no destino (modelo Tailscale) | funciona, mas a origem também deve negar cedo; a defesa em profundidade vale o custo do conntrack. |
| ACL só no host (nftables/iptables) | não é portátil para macOS, exige root e não é distribuída/auditável a partir de uma política única. |

### 12.4 Quando revisitar

- **>253 nós:** `/24` acaba; migrar para `/16` ou múltiplas redes.
- **relay saturado:** multi-região, peer relay ou QUIC.
- **throughput insuficiente:** kernel WireGuard no Linux atrás de uma trait; boringtun continua
  no macOS.
- **SSO/OIDC:** se a operação crescer para vários operadores; hoje join token + admin é
  suficiente.
- **IPv6 no overlay:** se o Seance ou os Macs exigirem; o underlay IPv6 pode ser testado antes.
- **HA:** se o control plane virar dependência crítica de novos builds; hoje o data plane
  sobrevive à queda.

---

## 13. Riscos e questões abertas

| # | Questão | Risco | Como fechar |
| --- | --- | --- | --- |
| 1 | Sinal de rendezvous para hole punching | B não dispara; NAT descarta A; direto falha sempre | implementar `punch` no mapa ou frame `PUNCH` no relay (§5.2) |
| 2 | Sucesso de NAT real | relay pode virar caminho comum | matriz de NAT em [`benchmarks.md`](benchmarks.md) |
| 3 | Capacidade/latência do relay | HOL, queda única | medir; multi-região se necessário |
| 4 | Privilégios do TUN no macOS | instalação/launchd, UX | definir instalador e modelo de confiança |
| 5 | Revogação de nó offline | janela stale | TTL de 1 h (reduzir), auditoria, rotação futura |
| 6 | Tamanho/fanout do mapa com 250 nós | payload e CPU | medir build/fanout; gzip/ETag se preciso |
| 7 | Conntrack da ACL (não implementado) | respostas TCP exigem regra espelhada; semântica UDP/ICMP | implementar limites/testes; medir |
| 8 | Skew de relógio | token rejeitado | `server_time`, tolerância, NTP |
| 9 | Backup/HA do SQLite | perda de estado | rotina de backup/restore testada |
| 10 | IPv6 no underlay | candidatos/relay | testar `SocketAddr` IPv6; não prometer |
| 11 | DNS (`wm.internal`) | operador depende de IP | resolver futuro ou hosts local |
| 12 | Multi-relay | SPOF | decidir só com medição |
| 13 | Rotação de chaves | comprometimento | procedimento de re-enroll |
| 14 | Observabilidade | difícil diagnosticar | métricas/logs estruturados; a definir |
| 15 | Integração com Seance | exposição de IP ao aluno | API read-only, server-side, revisão de auth |
| 16 | Privacidade do relay | metadados de quem fala com quem | TLS, retenção mínima, documentar |
| 17 | Forward de serviços em loopback (não implementado) | `10.66.0.N:8421` não abre; expor porta errada/ampla | allowlist explícita, ACL, teste de Host |
| 18 | Handshake init O(n) no engine | custo sob 250 peers/flood | `parse_handshake_anon` + HashMap; rate limit; medir |
| 19 | Recompilação de ACL em mudança de tag | regra `tag:` stale | recompilar no update/aprovação ou no build do mapa |
| 20 | Revogação de relay ativo | nó desabilitado relayando até 1 h | fechar via `RelayHub` no revoke |

---

## 14. Método de medição

Não há números de benchmark neste documento. O esqueleto, os cenários, os alvos preliminares e
as tabelas vazias estão em [`benchmarks.md`](benchmarks.md). Regras:

- medir no commit e no build (`--release`) registrados;
- mesma máquina/rede, com e sem `tc netem`, três execuções, reportar p50/p95/p99 e mediana;
- separar latência de handshake, throughput, RTT, memória e mapa de 250 nós;
- guardar dados brutos e comandos junto do resultado;
- não comparar números de máquinas/redes diferentes sem normalizar.

---

## 15. Referências primárias

### WireGuard e boringtun

- WireGuard — protocolo e timers: <https://www.wireguard.com/protocol/>
- WireGuard — paper (NDSS 2017): <https://www.wireguard.com/papers/wireguard.pdf>
- WireGuard — quickstart/`PersistentKeepalive`: <https://www.wireguard.com/quickstart/>
- boringtun 0.7.1 — `Tunn::new` e `index << 8`:
  <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/mod.rs#L209>
- boringtun 0.7.1 — `inc_index` (byte baixo):
  <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/handshake.rs#L466>
- boringtun 0.7.1 — demux `receiver_idx >> 8`:
  <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/device/mod.rs#L633>
- boringtun 0.7.1 — timers (`REKEY_AFTER_TIME`, `REJECT_AFTER_TIME`, `KEEPALIVE_TIMEOUT`):
  <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/timers.rs>
- Noise Protocol Framework: <https://noiseprotocol.org/noise.html>

### NAT, STUN e endereçamento

- RFC 8489 — STUN: <https://www.rfc-editor.org/rfc/rfc8489.html>
- RFC 4787 — NAT behavioral requirements: <https://www.rfc-editor.org/rfc/rfc4787.html>
- RFC 6598 — CGNAT `100.64.0.0/10`: <https://www.rfc-editor.org/rfc/rfc6598.html>
- RFC 8200 — IPv6, MTU mínimo 1280: <https://www.rfc-editor.org/rfc/rfc8200.html>

### Tailscale e Headscale

- How Tailscale works: <https://tailscale.com/blog/how-tailscale-works>
- How NAT traversal works: <https://tailscale.com/blog/how-nat-traversal-works>
- Control and data planes: <https://tailscale.com/docs/concepts/control-data-planes>
- Connection types (direto, DERP, peer relay): <https://tailscale.com/docs/reference/connection-types>
- DERP servers: <https://tailscale.com/docs/reference/derp-servers>
- ACLs (deny-by-default, locally enforced): <https://tailscale.com/kb/1018/acls>
- Endereços `100.x.y.z` (CGNAT): <https://tailscale.com/kb/1015/100.64.0.0>
- Cliente Tailscale — `/map` com long poll:
  <https://github.com/tailscale/tailscale/blob/main/control/controlclient/direct.go>
- Headscale — features e escopo: <https://headscale.net/stable/about/features/>
- Headscale — repositório: <https://github.com/juanfont/headscale>

### NetBird

- How NetBird works (management, signal, relay, WireGuard, 100.64/10):
  <https://docs.netbird.io/about-netbird/how-netbird-works>

### Nebula

- Nebula — README (Noise, lighthouses, certificados): <https://github.com/slackhq/nebula>
- Nebula — documentação: <https://nebula.defined.net/docs/>

### Código do wired-magic

- Tipos de mapa/ACL/relay: [`crates/wm-proto/src/lib.rs`](../crates/wm-proto/src/lib.rs)
- Schema, presença e alocação: [`crates/wm-control/src/db.rs`](../crates/wm-control/src/db.rs)
- Estado em memória, versão e relay token: [`crates/wm-control/src/state.rs`](../crates/wm-control/src/state.rs)
- API de mapa/enroll/endpoints: [`crates/wm-control/src/api/node.rs`](../crates/wm-control/src/api/node.rs)
- API admin/ACL: [`crates/wm-control/src/api/admin.rs`](../crates/wm-control/src/api/admin.rs),
  [`crates/wm-control/src/acl.rs`](../crates/wm-control/src/acl.rs)
- API de integração: [`crates/wm-control/src/api/integration.rs`](../crates/wm-control/src/api/integration.rs)
- Relay e STUN: [`crates/wm-control/src/api/relay_ws.rs`](../crates/wm-control/src/api/relay_ws.rs),
  [`crates/wm-control/src/relay.rs`](../crates/wm-control/src/relay.rs),
  [`crates/wm-control/src/stun.rs`](../crates/wm-control/src/stun.rs)
- Tokens e assinatura de relay: [`crates/wm-control/src/auth.rs`](../crates/wm-control/src/auth.rs)
- Configuração da malha: [`crates/wm-control/src/config.rs`](../crates/wm-control/src/config.rs)
- Data plane e agente: [`crates/wm-core/src/wireguard.rs`](../crates/wm-core/src/wireguard.rs),
  [`crates/wm-core/src/agent.rs`](../crates/wm-core/src/agent.rs),
  [`crates/wm-core/src/tun.rs`](../crates/wm-core/src/tun.rs)
- Control/STUN/relay clients: [`crates/wm-core/src/control.rs`](../crates/wm-core/src/control.rs),
  [`crates/wm-core/src/stun.rs`](../crates/wm-core/src/stun.rs),
  [`crates/wm-core/src/relay_client.rs`](../crates/wm-core/src/relay_client.rs)
- CLI/TUI: [`crates/wm/src/main.rs`](../crates/wm/src/main.rs),
  [`crates/wm-tui/src/lib.rs`](../crates/wm-tui/src/lib.rs)
- Workspace e dependências: [`Cargo.toml`](../Cargo.toml)
- ADRs: [`adr/0001-data-plane-boringtun.md`](adr/0001-data-plane-boringtun.md),
  [`adr/0002-control-plane-map.md`](adr/0002-control-plane-map.md),
  [`adr/0003-relay-transport.md`](adr/0003-relay-transport.md),
  [`adr/0004-overlay-addressing.md`](adr/0004-overlay-addressing.md),
  [`adr/0005-acl-and-integration.md`](adr/0005-acl-and-integration.md)
- Benchmarks: [`benchmarks.md`](benchmarks.md)

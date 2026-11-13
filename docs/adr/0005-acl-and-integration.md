# ADR 0005 — ACL distribuída e API read-only de integração com o Seance

- **Status:** Aceito; implementado (tabela de fluxos TCP/UDP/ICMP e recompilação por tag)
- **Data:** 2026-11-12
- **Decisores:** dono + pesquisa técnica
- **Relacionado:** [RESEARCH §8](../RESEARCH.md#8-acl-distribuída),
  [ADR 0002](0002-control-plane-map.md), [ADR 0004](0004-overlay-addressing.md)

## Contexto

A malha dá a cada Mac um IP de overlay. Sem política, qualquer nó ativo poderia falar com
qualquer Baguette, Xcode ou SSH. O operador precisa de uma política única, versionada e
auditável, que valha para todos os nós sem configurar firewall em cada Mac.

O Seance precisa listar os nós e seus IPs de overlay para mostrar ao operador “abrir o
Baguette deste Mac”. Quem usa o site é o aluno; ele não pode receber o IP de overlay, não tem
login no wired-magic e não fala com o Mac. A integração é server-side e read-only.

O `wm-proto` define `AclPolicy`, `AclRule`, `CompiledAcl`, `CompiledRule`, `AclAction`,
`SimulateRequest/Response`, `IntegrationNode`, `IntegrationNodesResponse` e
`CreateServiceTokenRequest` (escopo default `integration:nodes:read`).

## Decisão

### ACL

1. **Política YAML com seletores** `*`, `name:foo`, `tag:bar` e CIDR. Ações `accept`/`deny`;
   protocolo `tcp`/`udp`/`icmp`/`*`; portas `*`, `22`, `8000-8100`.
2. **Compilação no control plane.** Seletores viram IPs de overlay (`/32` por nó, CIDR para
   `*`/CIDR), protocolo vira número e portas viram `PortRange`; a ordem é preservada e cada par
   src/dst vira uma `CompiledRule` (limite de 8.192). O YAML e o `CompiledAcl` ficam em
   `acl_policies`; o mapa leva o compilado.
3. **First-match wins.** A primeira regra que casa decide. O compilador atual grava
   `default_allow = rules.is_empty()`: sem regras compiladas, allow (bootstrap); com pelo menos
   uma, deny-by-default. Se todos os seletores de uma regra não resolverem, ela vira inerte e
   pode deixar `rules` vazio — os warnings de `/admin/acl/validate` precisam ser tratados como
   erro de operação.
4. **Aplicação nos dois lados.** A mesma `CompiledAcl::is_allowed` é chamada:
   - no **egress**, antes de cifrar: `dst` do peer, proto/porta; o código usa `me.overlay_ip`
     como origem, mas ainda não compara `meta.src` com o IP local;
   - no **ingress**, depois de decifrar: `src == peer.overlay_ip` e `dst == meu IP`.
   Pacote negado é dropado; a origem não gasta cifra/rede, o destino não confia na origem.
5. **Anti-spoof** é parte da ACL no ingress (`src == peer.overlay_ip`); no egress a checagem
   explícita de `src == me.overlay_ip` ainda falta. O ingress do peer é a defesa final e
   substitui o `AllowedIPs` do WireGuard de kernel.
6. **Tabela de fluxos para respostas (implementada).** O matcher continua stateless por
   regra, mas o agente aprende fluxos aceitos por 5 min; a resposta TCP/UDP/ICMP de uma
   conexão estabelecida é liberada. Um conntrack completo (fragmentos, timeouts finos) não é
   uma tabela (TCP 300 s, UDP 60 s, ICMP echo por identificador, limite 4.096, LRU), com o
   primeiro pacote passando pela ACL nos dois lados e a resposta de fluxo estabelecido liberada.
7. **Propagação e falha.** A ACL vai no `NetworkMap`; mudança incrementa a versão. Se o control plane
   cai, o agente mantém a última ACL válida (não fail-closed por default). Fail-closed é opção
   futura para redes de alta segurança.
8. **Simulação e auditoria.** `PUT /api/v1/admin/acl` publica, `POST /api/v1/admin/acl/validate`
   compila com warnings e `POST /api/v1/admin/acl/simulate` usa a mesma função e devolve
   `allowed`, `rule_index` e `reason`; publicação de ACL gera `audit_events`.

Exemplo:

```yaml
version: 7
rules:
  - action: accept
    src: ["tag:operator"]
    dst: ["tag:mac"]
    protocol: tcp
    ports: ["8421"]
  - action: deny
    src: ["*"]
    dst: ["tag:mac"]
    protocol: "*"
    ports: ["*"]
```

### Integração read-only

1. **Rotas:** `GET /api/v1/integration/nodes` (filtros `tag`/`online`),
   `GET /api/v1/integration/nodes/{name}` e `GET /api/v1/integration/health`.
2. **Auth:** `Authorization: Bearer <service_token>`; token opaco com hash no banco, escopo
   `integration:nodes:read`, revogação, `last_used_at` e auditoria; rate limit é recomendado.
3. **Resposta:** `IntegrationNodesResponse { nodes, server_time }`, com
   `IntegrationNode { id, name, hostname, overlay_ip, tags, labels, online, last_seen }`.
4. **Nunca retorna:** chave WireGuard (pública ou privada), endpoints, token de relay, node token,
   `nat_type`, `peer_stats` ou qualquer segredo. O Seance não precisa disso.
5. **Uso no Seance:** server-side, para montar a lista de operadores. O IP de overlay é um
   **endpoint de operador**, não uma URL de aluno; o navegador do aluno nunca o recebe. Se o
   servidor do Seance não estiver na malha, ele só exibe o IP; quem acessa o Baguette é o
   dispositivo do operador, que tem um nó.
6. **Sem escrita no v1:** não há criar/remover nó, alterar ACL ou emitir token de relay por essa
   API. Isso é decisão separada, com escopo e auditoria próprios.

## Alternativas

| Alternativa | Prós | Contras | Veredito |
| --- | --- | --- | --- |
| **ACL compilada + dois lados + fluxos** | defesa em profundidade, uma política, auditoria | fluxos simplificados | **escolhido; implementado** |
| Sem ACL | simples | qualquer nó alcança qualquer Baguette | rejeitado |
| Firewall no host (nftables/iptables) | rápido, kernel | não portátil (macOS), root, não distribuído/auditável | rejeitado |
| ACL só no destino (modelo Tailscale) | simples, já provado | origem não nega cedo; sem defesa em profundidade | rejeitado |
| ACL só na origem | economiza rede | um nó comprometido/versão antiga injeta tráfego | rejeitado |
| Zero trust completo (mTLS por serviço, OIDC) | forte | complexidade e identidade que o Seance não tem | futuro, não v1 |
| Integração lendo o SQLite do control plane | sem API | acoplamento, risco de corrupção, sem escopo | rejeitado |
| Webhook/push para o Seance | tempo real | mais um estado/retry; o polling read-only basta | rejeitado no v1 |

## Consequências

- O agente mantém uma tabela de fluxos (5 min) para respostas TCP/UDP/ICMP; com
  deny-by-default, o primeiro pacote ainda precisa de regra. Um conntrack completo é a
  principal lacuna funcional da ACL.
- A política é direcional; documentar que `accept A -> B` não libera `B -> A` para fluxos
  novos. Quando o conntrack existir, ele cobre a resposta de um fluxo já aceito.
- Recompilar ao mudar nome/tag de nó; o `put_acl` compila no save, mas o `update_node` atual
  não recompila. Uma regra `tag:` pode ficar stale até a próxima publicação; corrigir no
  `update_node`/`approve` ou compilar no build do mapa.
- Revogação de nó remove a chave do mapa; peers atualizados rejeitam o handshake. A janela de
  mapa stale é o limite (§10.1 do RESEARCH).
- A API de integração é mais uma superfície pública; token de serviço, TLS, rate limit e
  allowlist de IP do Seance são recomendados. O token nunca vai para o bundle Next.
- O forward de serviços em loopback (Baguette) precisa de allowlist explícita de portas e é
  coberto pela ACL; a ACL sozinha não deve expor qualquer porta local.
- Testes: first-match, default, egress/ingress, anti-spoof, conntrack, simulação, escopo de
  token, resposta sem segredos.

## Critérios de aceite

- `CompiledAcl` com regra `deny` antes de `accept` nega o caso; a ordem é preservada.
- Pacote com `src` spoofado é dropado no ingress; adicionar a checagem no egress.
- TCP 8421 operator→mac funciona; a resposta é liberada pela tabela de fluxos.
- Regra `deny * -> tag:mac` bloqueia portas não listadas.
- Token sem `integration:nodes:read` recebe 403; token revogado recebe 401.
- JSON de integração não contém chave, endpoint ou token.
- Nenhum IP `10.66.x.x` aparece em resposta para o navegador do aluno.

## Referências

- Código: [`crates/wm-proto/src/lib.rs`](../../crates/wm-proto/src/lib.rs)
  (`AclPolicy`, `CompiledAcl`, `Simulate*`, `IntegrationNode`),
  [`crates/wm-control/src/acl.rs`](../../crates/wm-control/src/acl.rs),
  [`crates/wm-control/src/api/admin.rs`](../../crates/wm-control/src/api/admin.rs),
  [`crates/wm-control/src/api/integration.rs`](../../crates/wm-control/src/api/integration.rs),
  [`crates/wm-core/src/agent.rs`](../../crates/wm-core/src/agent.rs)
- Tailscale ACLs — deny-by-default, locally enforced:
  <https://tailscale.com/kb/1018/acls>
- Tailscale — política distribuída e filtro no nó:
  <https://tailscale.com/blog/how-tailscale-works>
- NetBird — ACL aplicada no cliente:
  <https://docs.netbird.io/about-netbird/how-netbird-works>

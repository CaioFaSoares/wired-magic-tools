# ADR 0004 — Endereçamento do overlay: 10.66.0.0/24, .1 e MTU 1280

- **Status:** Aceito; implementado (forward do Baguette pendente)
- **Data:** 2026-11-12
- **Decisores:** dono + pesquisa técnica
- **Relacionado:** [RESEARCH §9](../RESEARCH.md#9-endereçamento-dns-e-mtu),
  [ADR 0001](0001-data-plane-boringtun.md), [ADR 0005](0005-acl-and-integration.md)

## Contexto

Cada nó precisa de um IP de overlay estável, que o operador e a integração com o Seance possam
usar. O IP não pode colidir com a LAN do Mac, com um Tailscale já instalado, nem com a faixa de
outra VPN. O Seance já usa `10.66.0.0/24` na pesquisa R11/D022 e no control plane de malha
dele; a integração fica mais simples se os dois falarem a mesma faixa.

O `wm-proto` define `DEFAULT_OVERLAY_CIDR = "10.66.0.0/24"`, `DEFAULT_MTU = 1280`,
`DEFAULT_DNS_ZONE = "wm.internal"` e `NetworkInfo.server_address`. O `config.rs` valida o CIDR,
reserva `.1` como `server_ip()` e aceita `overlay_cidr` com prefixo de rede ≤ `/24` (por exemplo, `/24` ou `/16`).

## Decisão

1. **Overlay `10.66.0.0/24`.**
   - `.0`: rede.
   - `.1`: identidade reservada do control plane/gateway; **não é um peer WireGuard no v1**,
     não é próximo salto e não recebe tráfego de dados.
   - `.2`–`.254`: nós. São 253 endereços.
   - `.255`: broadcast.
2. **Alocação determinística pelo menor IP livre.** `create_node` percorre `net.hosts()`, pula
   `.1` e o broadcast, e escolhe o primeiro livre; o IP fica no SQLite e é estável.
3. **MTU 1280 no TUN.** A conta é 1280 + 16 (header WG) + 16 (tag) + 8 (UDP) + 20/40 (IP
   externo) = 1340/1360 no fio; cabe em underlay 1500 e atende ao mínimo IPv6 de 1280. O TCP
   negocia MSS a partir do MTU do TUN; não há fragmentação no WireGuard.
4. **Rota `10.66.0.0/24` para o TUN, sem default route.** O agente não é exit node, não faz
   subnet routing e não encaminha pacotes entre peers.
5. **`wm.internal` é metadado no v1.** `dns_name`/`dns_zone` existem para a UI e para um futuro
   resolver; não há MagicDNS nem nameserver distribuído.
6. **CIDR configurável, mas default fixo.** O operador pode trocar `overlay_cidr` se a LAN
   usar `10.66.0.0/24`; a integração com o Seance assume o default.

## Por que não `100.64.0.0/10`

- Tailscale e NetBird usam `100.64.0.0/10` por padrão. Um Mac com Tailscale instalado (o host
  de desenvolvimento tem) teria duas VPNs disputando a mesma faixa; a rota de uma pode capturar
  o tráfego da outra.
- O Seance já usa `10.66.0.0/24`; manter a faixa evita tradução na integração e na documentação.
- `10.66.0.0/24` é RFC 1918 e não é anunciado na internet; o TUN só recebe a rota específica.
- O risco de colisão com LAN existe em qualquer faixa privada; `10.66` é menos comum que
  `10.0.0.0/8` ou `192.168.0.0/16`, e é configurável.

## Alternativas

| Alternativa | Prós | Contras | Veredito |
| --- | --- | --- | --- |
| **10.66.0.0/24** | combina com o Seance; evita CGNAT do Tailscale; simples | pode colidir com uma LAN `10.66.0.0/24`; 253 nós | **escolhido** |
| `100.64.0.0/10` | padrão Tailscale/NetBird; amplo | colide com Tailscale; integração teria que traduzir | rejeitado |
| `10.0.0.0/8` | amplo | colide com LANs domésticas/corporativas; rota ampla | rejeitado |
| `172.16.0.0/12` | privado, menos comum | ainda colide; integração diferente do Seance | rejeitado |
| IPv6 ULA (`fd00::/8`) | espaço, sem NAT | macOS/underlay IPv6, complexidade; fora do v1 | futuro |
| `/24` aleatório por rede | evita colisão | difícil documentar/integrar; IPs mudam por instalação | rejeitado no v1 |

## Consequências

- O limite é 253 nós. Acima disso, migrar para `/16` ou múltiplas redes; o código já aceita
  prefixo ≤ `/24`, mas o default e a integração são `/24`.
- `.1` reservado pode confundir: não é pingável no v1 e não é gateway. A UI/documentação devem
  dizer “identidade do control plane”, não “router”.
- MTU 1280 reduz o payload útil em relação a 1420/1440; o ganho é evitar black hole de MTU.
  O throughput é medido em [`benchmarks.md`](../benchmarks.md).
- Se a LAN usar `10.66.0.0/24`, o operador troca o CIDR ou a LAN; a integração com o Seance
  precisa saber que o default mudou.
- **IP de overlay não expõe serviço em loopback.** O Baguette escuta `127.0.0.1:8421`; o TUN
  entrega o pacote ao Mac, mas não há socket em `10.66.0.N:8421`. O agente atual **não tem**
  forward; é preciso um forward allowlisted (`10.66.0.N:8421 -> 127.0.0.1:8421`) ou bindar o
  Baguette no overlay; validar também o Host allowlist do Baguette. Isso é decisão de
  integração, não de endereçamento.
- Sem IPv6 no overlay no v1; o `SocketAddr` de endpoint aceita IPv6, mas não é prometido.

## Critérios de aceite

- Alocação nunca entrega `.0`, `.1` ou `.255`; 253 nós cabem; o 254º falha com erro claro.
- Config rejeita prefixo > `/24` e MTU fora de 576..1500.
- Rota do TUN é só `10.66.0.0/24`; `ping 10.66.0.1` não é usado como gateway.
- Integração do Seance recebe `overlay_ip` `10.66.0.N` e não vaza para o navegador.
- Teste de MTU: pacote de 1280 passa direto e via relay sem fragmentação.
- Baguette acessível em `http://10.66.0.N:8421` via forward allowlisted (a implementar); portas
  não listadas não são expostas; Host aceito pelo Baguette.

## Referências

- Código: [`crates/wm-proto/src/lib.rs`](../../crates/wm-proto/src/lib.rs)
  (`DEFAULT_OVERLAY_CIDR`, `DEFAULT_MTU`),
  [`crates/wm-control/src/config.rs`](../../crates/wm-control/src/config.rs) (`server_ip`,
  validação),
  [`crates/wm-control/src/db.rs`](../../crates/wm-control/src/db.rs) (`create_node`)
- Tailscale — `100.64.0.0/10`: <https://tailscale.com/kb/1015/100.64.0.0>
- NetBird — `100.64.0.0/10` por conta:
  <https://docs.netbird.io/about-netbird/how-netbird-works>
- RFC 6598 — CGNAT: <https://www.rfc-editor.org/rfc/rfc6598.html>
- RFC 8200 — MTU mínimo IPv6 1280: <https://www.rfc-editor.org/rfc/rfc8200.html>

# ADR 0003 — Relay WSS na 443, frames binários e token assinado

- **Status:** Aceito; implementado com lacunas
- **Data:** 2026-11-12
- **Decisores:** dono + pesquisa técnica
- **Relacionado:** [RESEARCH §7](../RESEARCH.md#7-relay-wss),
  [ADR 0001](0001-data-plane-boringtun.md), [ADR 0002](0002-control-plane-map.md)

## Contexto

Quando o caminho UDP direto não funciona (NAT simétrico, firewall que bloqueia UDP, CGNAT
agressivo), o wired-magic precisa de um fallback que atravesse a rede e não vaze plaintext.
O VPS já tem 443/TCP aberto para o control plane; não queremos um segundo serviço, uma segunda
porta ou uma segunda conta.

O relay deve encaminhar pacotes WireGuard entre nós autenticados, sem decifrar, e ser barato o
suficiente para rodar no mesmo processo do control plane. O `wm-proto` define `RelayFrame`,
`RelayClaims` e `RELAY_MAX_PAYLOAD`; o servidor WSS, o `RelayHub` e o cliente do agente já
existem. As lacunas são revogação de conexão ativa e timeout explícito de Pong.

## Decisão

1. **WSS na 443, path `/relay`, no mesmo host do control plane.** TLS com o mesmo certificado;
   um listener axum; sem DNS/porta extra. O relay roda no mesmo binário (`relay_enabled`,
   `relay_region`).
2. **Frames binários**, sem JSON/base64 por pacote. Primeiro byte é a tag; UUID de 16 bytes;
   payload opaco. Tabela:

   | Tag | Frame | Direção | Conteúdo |
   | --- | --- | --- | --- |
   | `0x01` | `Data` | cliente → relay | `dst` UUID + ciphertext |
   | `0x02` | `Deliver` | relay → cliente | `src` UUID + ciphertext |
   | `0x03` | `Ping` | ambos | vazio |
   | `0x04` | `Pong` | ambos | vazio |
   | `0x05` | `Presence` | relay → cliente | `node` UUID + online |
   | `0x06` | `Error` | relay → cliente | UTF-8 |

   `RELAY_MAX_PAYLOAD = 64 KiB`. O `encode` rejeita acima disso; o servidor impõe o limite no
   read do WebSocket (o `decode` sozinho não rejeita).
3. **Token de relay Ed25519**, `sub` = nó, `aud` = URL exata do relay, `iat`/`exp`. Formato
   `base64url(payload).base64url(assinatura)`, sem header JWT. TTL atual **1 h** (o
   `sign_relay_token` permite 60 s..24 h); o alvo de endurecimento é 15 min. Enviado no header
   `Authorization: Bearer`, não na query string.
4. **Uma conexão por nó.** O relay mantém `DashMap<NodeId, Sender>`; uma nova conexão para o
   mesmo `sub` substitui o registro e a antiga tende a cair. Não há estado persistente.
5. **Roteamento por UUID.** O relay valida que `dst` é um nó ativo do mapa e que o `src` do
   `Deliver` é o `sub` autenticado; não há broadcast. Ele não lê o ciphertext.
6. **Ping/Pong**: o servidor manda `Ping` a cada 25 s; o cliente manda a cada 20 s. O cliente
   atual não fecha por Pong ausente; fecha em erro/close. Backpressure por canal de 512 frames
   no relay e 2.048 no cliente; `try_send` falha e o frame é descartado, sem bloquear o leitor.
7. **Fallback explícito.** O agente só usa o relay se a tentativa direta falhar ou se o relay
   já estiver conectado e o caminho direto cair. O relay não é caminho primário.
8. **Revogação:** o token é validado no upgrade, mas o `revoke_node` atual **não fecha** a
   conexão WSS ativa; o nó desabilitado pode continuar relayando até a conexão cair ou o token
   expirar (1 h). Fechar via `RelayHub` no revoke é uma correção necessária. Se o relay for
   separado no futuro, precisa de introspecção/revogação.

## Por que não QUIC/TCP bruto no v1

| Alternativa | Por quê não |
| --- | --- |
| **QUIC** | Depende de UDP 443, que é exatamente o que muitas redes bloqueiam. O fallback precisa do caminho mais compatível (443/TCP/TLS). QUIC adiciona complexidade de transporte e multiplexação sem resolver esse caso. Revisitar se o relay virar caminho de throughput. |
| **TCP bruto/TLS custom** | Não atravessa proxy corporativo como WSS; exige framing e autenticação próprios; não traz benefício sobre WebSocket. |
| **TURN/Coturn** | Mais um serviço, alocações, auth e keepalive; TURN sobre TCP sofre do mesmo HOL; não precisamos de alocação porque já temos control plane e token. |
| **DERP** | Protocolo próprio da Tailscale (código aberto, mas específico), implementação Go, multi-região; copiamos a ideia (relay de ciphertext na 443), não o protocolo. |
| **HTTP POST por frame** | Overhead request/response e HOL; não é bidirecional persistente. |
| **WebRTC data channel** | ICE/DTLS/SCTP demais para um relay de pacotes; o agente já tem WireGuard. |

## Consequências

- TCP impõe head-of-line blocking; o relay é bom para Baguette/controle, não para throughput
  alto. A matriz de benchmark compara direto vs relay.
- Um relay é SPOF do caminho relayado. Multi-região/failover é trabalho futuro; o v1 tem uma
  região (`relay_region`).
- O relay vê metadados (quem fala com quem, quando, tamanho); não vê conteúdo. TLS protege o
  token e o ciphertext em trânsito.
- O token de 1 h é renovado a cada mapa (25 s), mas a conexão existente não é reaberta; em
  reconexão o cliente usa o `RelayInfo` mais recente. Se o control plane estiver fora, o token
  atual continua válido até o `exp`.
- O relay e o control plane no mesmo processo simplificam revogação, mas acoplam o deploy; se
  o relay saturar, separar é uma decisão de capacidade.
- Se houver proxy reverso, ele precisa permitir `Upgrade`/`Connection` do WebSocket e ter idle
  timeout maior que o ping (20 s); buffering de resposta quebra o relay.

## Critérios de aceite

- Com UDP bloqueado, dois agentes trocam tráfego pelo relay e o Baguette responde.
- Token com `aud` errada, assinatura inválida ou expirado é rejeitado.
- Frame acima de 64 KiB é rejeitado sem alocar memória desproporcional.
- Queda do WSS: agente reconecta com backoff, sem duplicar conexão.
- Relay não consegue decifrar o payload (teste com ciphertext aleatório).
- Backpressure não trava o event loop; drop é contabilizado.

## Referências

- Código: [`crates/wm-proto/src/lib.rs`](../../crates/wm-proto/src/lib.rs)
  (`RelayFrame`, `RelayClaims`, `RELAY_MAX_PAYLOAD`),
  [`crates/wm-control/src/auth.rs`](../../crates/wm-control/src/auth.rs)
  (`sign_relay_token`, `verify_relay_token`),
  [`crates/wm-control/src/relay.rs`](../../crates/wm-control/src/relay.rs),
  [`crates/wm-control/src/api/relay_ws.rs`](../../crates/wm-control/src/api/relay_ws.rs),
  [`crates/wm-core/src/relay_client.rs`](../../crates/wm-core/src/relay_client.rs)
- Tailscale DERP (relay de ciphertext na 443): <https://tailscale.com/docs/reference/derp-servers>
- Tailscale connection types: <https://tailscale.com/docs/reference/connection-types>
- RFC 6455 — WebSocket: <https://www.rfc-editor.org/rfc/rfc6455.html>

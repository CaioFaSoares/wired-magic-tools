# ADR 0001 — Data plane com boringtun userspace + TUN

- **Status:** Aceito; implementado com lacunas
- **Data:** 2026-11-12
- **Decisores:** dono + pesquisa técnica
- **Relacionado:** [RESEARCH §6](../RESEARCH.md#6-wireguard-no-data-plane),
  [ADR 0003](0003-relay-transport.md)

## Contexto

O wired-magic precisa de um data plane WireGuard que rode em Linux e macOS, dentro do agente,
sem módulo de kernel específico de plataforma e com controle sobre UDP, STUN, relay e ACL. O agente é o
único componente privilegiado (cria TUN); o control plane não tem `CAP_NET_ADMIN`.

O workspace é Rust (`edition 2024`), e o `Cargo.toml` declara `boringtun = "0.7.1"` e
`tun = "0.8.14"` (feature `async`). O `wm-core` já implementa `WgEngine`, TUN, control client,
STUN client, relay client, ACL e API local; o boringtun é biblioteca, não daemon. O que falta é
handshake init O(1), conntrack e o forward do Baguette.

## Decisão

1. **boringtun 0.7.1 em userspace** para o protocolo WireGuard, com o crate `tun` para o
   dispositivo TUN (`utun` no macOS, `tun`/`tun0` no Linux).
2. **Uma `Tunn` por peer**, não uma `Tunn` global. O `WgEngine` mantém
   `HashMap<NodeId, WgPeer>` e `HashMap<u32, NodeId>`; cada `WgPeer` tem sua `Tunn`, endpoint,
   contadores e fila. O engine é dono único no `data_loop` (sem `DashMap`/lock por peer).
3. **Slot de 24 bits único por peer.** O agente passa `slot` (`next_slot`, 1..=0xFFFFFF) para
   `Tunn::new`, que internamente grava `slot << 8`. O byte baixo é a sessão, incrementado por
   `inc_index` sem tocar nos 24 bits altos. O slot não é o IP de overlay; o `/24` limita a 253
   peers, mas o slot é um identificador de demux independente. Data/response/cookie usam
   `receiver_idx >> 8`; não se varre a lista de peers.
4. **Handshake init é a exceção.** Data/response/cookie usam `receiver_idx >> 8` e são O(1).
   O `sender_index` do init pertence ao iniciador; o engine atual tenta o peer do endpoint
   cache e depois **todos** os peers (O(n)). A otimização recomendada é
   `parse_handshake_anon(static_private, static_public, packet)` + `HashMap<[u8;32], NodeId>`,
   com um `RateLimiter` compartilhado (MAC1/cookie) antes do parse. **Ainda não implementada.**
5. **Timers próprios.** Um timer de 100 ms manda `Cmd::Timer` ao `data_loop`, que chama
   `update_timers` em cada `Tunn`, drena `WriteToNetwork` e envia; `PersistentKeepalive = 25 s`
   para peers atrás de NAT.
6. **Roaming só após autenticação.** O endpoint do peer é atualizado apenas quando um pacote
   passa em `decapsulate`/handshake; nunca pelo IP de origem de um pacote não autenticado.
7. **MTU 1280** no TUN, rota `10.66.0.0/24`; sem default route, sem exit node, sem forwarding.
8. **Privilégio no agente.** Criar TUN exige root/CAP_NET_ADMIN no host; o control plane
   continua sem privilégio. O instalador/launchd do agente é decisão de deploy, não deste ADR.

## Alternativas

| Alternativa | Prós | Contras | Veredito |
| --- | --- | --- | --- |
| **boringtun + TUN** | Rust, biblioteca, roda macOS/Linux, controle do socket/relay/ACL | cópias userspace, timers/demux por nossa conta, fila de 256 pacotes por peer | **escolhido** |
| WireGuard de kernel (Linux) | rápido, maduro, menos CPU | não existe no macOS; menos controle sobre relay/STUN; exige netlink/`wg` | fallback futuro no Linux |
| `wireguard-go` + TUN | referência, multi-peer | Go/FFI, daemon separado, difícil integrar relay/ACL; dependência grande | rejeitado |
| `boringtun::device::Device` | multi-peer, UAPI, socket/TUN prontos | dono do socket UDP e do TUN; difícil plugar relay WSS, STUN e ACL própria | rejeitado |
| Tailscale/`tsnet` | NAT traversal e DERP prontos | Go, stack inteira, pouco controle, terceiro serviço | rejeitado |
| Proxy TCP por aplicação | simples, sem TUN | não é L3, não serve para Baguette/Xcode/SSH, um proxy por porta | rejeitado |

## Fallback e escape

- **Transporte:** se o UDP direto falhar, o peer usa o relay WSS ([ADR 0003](0003-relay-transport.md)).
  Isso não muda a `Tunn`; o agente alimenta o ciphertext do frame `Deliver`.
- **Implementação:** se o boringtun não atingir o throughput/CPU necessário no Linux, a saída é
  uma trait `DataPlane` com backend de kernel WireGuard no Linux e boringtun no macOS. Não é
  v1; exige benchmark e uma decisão própria.
- **TUN indisponível:** sem TUN não há overlay L3. O agente pode continuar conectado ao
  control plane e ao relay para diagnóstico, mas não promete acesso ao Baguette. Não inventamos
  um “modo relay-only” que finge ser data plane.

## Consequências

- O engine atual é dono único do `WgEngine` em um `data_loop` serializado (sem locks por peer);
  isso simplifica o estado, mas limita o caminho a um núcleo. Medir e, se necessário, shardar.
- O `WgEngine` aloca buffer por pacote (`vec![0u8; 2048]`, `to_vec`); é um custo a medir e
  otimizar. O boringtun já cobre o protocolo e o rate limiter de handshake.
- O handshake init é O(n) no engine atual; sob 250 peers/flood, medir e implementar o lookup
  por chave estática.
- `apply_peers` atualiza metadados, mas não recria a `Tunn` se a chave pública de um peer
  existente mudar; rotação exige remover/readicionar o nó ou reiniciar o agente.
- O custo de CPU tende a ser maior que o kernel; o benchmark compara direto/relay e, se
  possível, kernel no Linux. Não há número neste ADR.
- A memória por peer inclui 8 sessões e uma fila de até 256 pacotes; o pior caso teórico com
  250 peers e MTU 1280 é ~78 MiB (81,9 MB) de payload de filas, se todas encherem. Medir e, se necessário,
  limitar tráfego por peer.
- O agente é privilegiado; o control plane não. A separação de privilégio é um invariante.

## Critérios de aceite

- Handshake e `ping` de overlay entre dois agentes, direto e via relay.
- `iperf3` sobre o TUN em Linux e macOS, com MTU 1280, sem fragmentação.
- Nenhum panic/UB com pacotes truncados/aleatórios; drops contabilizados.
- Endpoint muda após roaming e o tráfego continua sem reenroll.
- Memória e CPU medidos com 2, 50 e 250 peers.

## Referências

- Código: [`crates/wm-core/src/wireguard.rs`](../../crates/wm-core/src/wireguard.rs),
  [`crates/wm-core/src/tun.rs`](../../crates/wm-core/src/tun.rs),
  [`crates/wm-core/src/agent.rs`](../../crates/wm-core/src/agent.rs)
- `Cargo.toml` do workspace: [`Cargo.toml`](../../Cargo.toml)
- boringtun `Tunn::new`: <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/mod.rs#L209>
- boringtun `inc_index`: <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/noise/handshake.rs#L466>
- boringtun demux: <https://github.com/cloudflare/boringtun/blob/boringtun-0.7.1/boringtun/src/device/mod.rs#L633>
- WireGuard protocol: <https://www.wireguard.com/protocol/>

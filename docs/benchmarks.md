# Benchmarks do wired-magic

**Status:** esqueleto de medição. Nenhum número foi medido ainda; todas as células de resultado
são **a medir**. Os valores em “alvo preliminar” são hipóteses de produto para orientar a
medição, não resultados e não compromissos.

**Data:** 2026-11-12
**Método e desenho:** [RESEARCH.md](RESEARCH.md)

O agente e o control plane já têm CLI/API para instrumentar a medição: `wm status --json`,
`wm peers --json`, `wm ping <peer>`, `wm probe <peer> <porta>`, `wm resolve <nome>` e
`wm doctor`. Use essas saídas para registrar versão, `map_version`, `path`, `rtt_ms`,
`last_handshake_secs`, `control_connected` e `relay_connected` sem inventar números.

## 1. Regras

- Medir sempre no mesmo commit, build `--release`, com a mesma topologia e o mesmo hardware.
- Registrar commit, versão do macOS/Linux, modelo do Mac, CPU/RAM, link, provedor/região do VPS.
- Três execuções por cenário; reportar mediana, p50, p95 e p99. Não escolher a melhor execução.
- Aquecimento de 30 s antes de medir; duração de 60 s para throughput; 1.000 amostras para RTT.
- Congelar atualizações de SO, backups, indexação e outros agentes de rede.
- Medir com e sem `tc netem` (RTT/loss); não comparar máquinas/redes diferentes sem normalizar.
- Guardar dados brutos, comandos e config junto do resumo. Sem isso, o número não é publicável.
- Separar claramente: latência de handshake, throughput, RTT, memória, mapa de 250 nós.
- Não usar números de terceiros como se fossem nossos; referência externa é referência, não
  medição do wired-magic.

## 2. Testbed

Preencher na primeira execução:

| Item | Valor |
| --- | --- |
| Commit | a medir |
| Build | a medir (`cargo build --release`) |
| VPS (provedor/região/CPU/RAM/link) | a medir |
| macOS A (modelo/chip/memória/versão) | a medir |
| macOS B (modelo/chip/memória/versão) | a medir |
| Linux A (kernel/CPU/link) | a medir |
| Linux B (kernel/CPU/link) | a medir |
| Rede dos Macs | a medir |
| Data/hora (NTP) | a medir |

Cenários de rede:

| ID | Cenário | Como montar | Rota esperada |
| --- | --- | --- | --- |
| N1 | mesma LAN, 1 GbE | dois Macs no mesmo switch | direto (`lan`) |
| N2 | duas redes, NAT cone | dois roteadores com MASQUERADE | direto (`stun`) |
| N3 | uma rede simétrica | NAT com portas aleatórias por destino | tentativa direta; relay |
| N4 | UDP bloqueado | firewall dropa UDP 41641/3478 | relay |
| N5 | WAN com RTT 80 ms / perda 2% | `tc netem` no Linux | direto ou relay |
| N6 | relay-only | forçar `PathKind::Relay` | relay |

## 3. Handshake

**O que medir:** do primeiro `encapsulate` que gera uma handshake initiation até o primeiro
pacote autenticado entregue ao TUN (`WriteToTunnel`) no outro lado. Medir com sessão fria
(sem sessão), 100 iterações, p50/p95/p99. Separar direto e relay.

**Como:**

1. Dois agentes com peers configurados; `tcpdump`/log com timestamp monotônico nos dois lados.
2. Iniciar tráfego de um pacote (ex.: `ping -c 1`) e correlacionar pelo ID do peer.
3. Repetir após expirar a sessão (ou reiniciar o agente) para garantir sessão fria.
4. Para relay, forçar `PathKind::Relay` (N6) e repetir.
5. Registrar também o tempo até o primeiro `Pong`/resposta ICMP, que inclui ida e volta.

| Cenário | Rota | p50 (ms) | p95 (ms) | p99 (ms) | Amostras | Status |
| --- | --- | --- | --- | --- | --- | --- |
| N1 mesma LAN | direto | a medir | a medir | a medir | 100 | a medir |
| N2 NAT cone | direto | a medir | a medir | a medir | 100 | a medir |
| N3 NAT simétrico | direto (se der) | a medir | a medir | a medir | 100 | a medir |
| N4 UDP bloqueado | relay | a medir | a medir | a medir | 100 | a medir |
| N5 RTT 80 ms / 2% | direto | a medir | a medir | a medir | 100 | a medir |
| N5 RTT 80 ms / 2% | relay | a medir | a medir | a medir | 100 | a medir |
| N6 relay-only | relay | a medir | a medir | a medir | 100 | a medir |

Alvo preliminar: handshake direto p95 < 500 ms em WAN na mesma região; relay p95 < 1.000 ms.
Calibrar com o dono; não é um requisito fechado.

## 4. Throughput

**O que medir:** TCP e UDP sobre o overlay, direto vs relay, com MTU 1280. Reportar Mbps/Gbps,
CPU do agente (user+sys), CPU do relay e retransmissões. Testar 1 fluxo e 8 fluxos.

**Como:**

1. `iperf3 -s` no Mac B, `iperf3 -c 10.66.0.3` no Mac A (TCP), 60 s, 3 execuções.
2. `iperf3 -u -b 1G` para UDP; reportar perda e jitter.
3. Repetir em N1 (LAN), N2 (WAN) e N6 (relay-only).
4. Coletar CPU com `pidstat`/`top`/`powermetrics` (macOS) durante a medição.
5. Anotar se o link do VPS é o gargalo (relay).

| Cenário | Rota | TCP 1 fluxo (Mbps) | TCP 8 fluxos (Mbps) | UDP (Mbps) | Perda UDP | CPU agente | CPU relay | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| N1 LAN | direto | a medir | a medir | a medir | a medir | a medir | — | a medir |
| N2 WAN | direto | a medir | a medir | a medir | a medir | a medir | — | a medir |
| N6 relay-only | relay | a medir | a medir | a medir | a medir | a medir | a medir | a medir |
| N5 RTT 80 ms / 2% | direto | a medir | a medir | a medir | a medir | a medir | — | a medir |
| N5 RTT 80 ms / 2% | relay | a medir | a medir | a medir | a medir | a medir | a medir | a medir |

Alvo preliminar: direto ≥ 500 Mbps em link 1 GbE; relay ≥ 50 Mbps no VPS de teste. O relay é
fallback; o alvo pode ser menor que o direto.

## 5. RTT

**O que medir:** RTT do overlay (`ping`/`wm ping`) e RTT do underlay, no mesmo momento. Reportar
min/mediana/p95/p99 e perda. 1.000 pacotes, intervalo 100 ms.

| Cenário | Rota | Underlay p50 (ms) | Overlay min (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Perda | Status |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| N1 LAN | direto | a medir | a medir | a medir | a medir | a medir | a medir | a medir |
| N2 WAN | direto | a medir | a medir | a medir | a medir | a medir | a medir | a medir |
| N6 relay-only | relay | a medir | a medir | a medir | a medir | a medir | a medir | a medir |
| N5 RTT 80 ms / 2% | direto | a medir | a medir | a medir | a medir | a medir | a medir | a medir |

Alvo preliminar: overhead do direto < 2 ms vs underlay na mesma LAN; relay adiciona o RTT do
VPS + HOL. Medir o overhead, não só o RTT absoluto.

## 6. Memória

**O que medir:** RSS do agente e do control plane em três estados: idle 1 h, 250 peers, 10.000
fluxos. Medir também o tamanho do SQLite/WAL e a memória do relay sob carga.

**Como:**

1. `ps -o rss`/`footprint` no macOS; `/proc/<pid>/status` no Linux; amostrar a cada 10 s.
2. Estado idle: agentes conectados, sem tráfego, 1 h.
3. Estado 250 peers: simulador/containers com 250 nós; mapa completo; sem tráfego.
4. Estado 10k fluxos: `iperf3`/pktgen com muitas 5-tuples; medir o matcher stateless atual
   (o conntrack ainda não existe).
5. Comparar com/sem relay e com/sem ACL.

| Componente | Idle 1 h | 250 peers | 10k fluxos | Observação | Status |
| --- | --- | --- | --- | --- | --- |
| Agente macOS (RSS) | a medir | a medir | a medir | — | a medir |
| Agente Linux (RSS) | a medir | a medir | a medir | — | a medir |
| Control plane (RSS) | a medir | a medir | a medir | — | a medir |
| Relay (RSS) | a medir | a medir | a medir | — | a medir |
| SQLite + WAL | a medir | a medir | a medir | — | a medir |

Alvo preliminar: agente < 150 MB RSS com 250 peers; control plane < 200 MB. Atenção ao pior
caso teórico do payload das filas do boringtun (250 × 256 × 1280 ≈ 78 MiB / 81,9 MB); medir,
não assumir. O engine atual também aloca buffer por pacote; medir o efeito em pps alto.

## 7. Mapa com 250 nós

**O que medir:** tamanho do `NetworkMap` JSON, tempo de build no control plane, fanout (tempo
da mutação até o último agente receber), CPU/RSS do control plane, taxa de escritas no SQLite e
comportamento de reconexão após reinício.

**Como:**

1. Subir 250 agentes de teste (containers/processos), cada um com chave e node token únicos.
2. Medir o tempo de `build_map()` (instrumentação) e o tamanho do JSON (bytes).
3. Criar/renomear um nó e medir a latência até todos os 249 outros receberem a versão nova.
4. Medir CPU/RSS do control plane em regime (heartbeats de 25 s) e durante uma mudança.
5. Medir o efeito de `last_seen` a cada 25 s (~10 req/s, ~20 UPDATEs/s) no WAL.
6. Reiniciar o control plane e medir o pico de reconexões/CPU (backoff 1–30 s, sem jitter no
   código atual).
7. Medir o custo de **devolver o mapa completo a cada 25 s** para cada nó; é o comportamento
   atual, não um delta/204.

| Métrica | Alvo preliminar | Medido | Status |
| --- | --- | --- | --- |
| Tamanho do mapa (250 nós, com ACL) | a definir; medir | a medir | a medir |
| `build_map` p50/p95 | p95 < 50 ms | a medir | a medir |
| Fanout p95 (mutação → último agente) | < 2 s | a medir | a medir |
| CPU do control (regime) | < 10% de 2 vCPU | a medir | a medir |
| RSS do control (250 nós) | < 200 MB | a medir | a medir |
| WAL/escritas por segundo | medir | a medir | a medir |
| Reconexão pós-restart (250) | sem pico > 2× | a medir | a medir |
| Handshake init O(n) a 250 peers | medir p95/p99 | a medir | a medir |

## 8. Matriz de NAT / sucesso do caminho direto

**O que medir:** em cada combinação, quantas tentativas (20) terminam em direto, em quanto tempo,
e quantas caem para relay. Reportar taxa e IC95% se possível.

**Como:** montar NATs com Linux network namespaces + `iptables` MASQUERADE; para simétrico,
usar mapeamento de porta aleatório por destino; para UDP bloqueado, `DROP` em 41641/3478.
Cada tentativa começa com sessão fria e um `ping` de overlay.

| NAT A | NAT B | Direto (n/20) | Tempo até direto p50 | Caiu para relay | Status |
| --- | --- | --- | --- | --- | --- |
| cone | cone | a medir | a medir | a medir | a medir |
| cone | simétrico | a medir | a medir | a medir | a medir |
| simétrico | simétrico | a medir | a medir | a medir | a medir |
| UDP bloqueado | cone | a medir | a medir | a medir | a medir |
| CGNAT | cone | a medir | a medir | a medir | a medir |

Alvo qualitativo: a maioria dos pares domésticos (cone/cone e CGNAT/cone) deve conseguir direto;
simétrico/simétrico pode ficar em relay. O número real decide se o relay precisa de mais
capacidade.

## 9. ACL

**O que medir:** custo do matcher stateless (e do conntrack, quando existir) no caminho direto:
pps, latência adicionada e memória, com 0, 100 e 1.000 regras; 1.000 e 10.000 fluxos; primeiro
pacote vs resposta.

**Como:** `pktgen`/`iperf3` com 5-tuples variadas; comparar ACL desligada, ACL allow-all e ACL
deny-by-default; medir CPU do agente. O matcher atual é stateless; quando o conntrack entrar,
medir separadamente o custo da tabela e a taxa de acerto.

| Regras | Fluxos | pps | Latência adicionada p50 | Memória (conntrack futuro) | Status |
| --- | --- | --- | --- | --- | --- |
| 0 | 1.000 | a medir | a medir | a medir | a medir |
| 100 | 1.000 | a medir | a medir | a medir | a medir |
| 1.000 | 1.000 | a medir | a medir | a medir | a medir |
| 100 | 10.000 | a medir | a medir | a medir | a medir |

## 10. Relay

**O que medir:** throughput de frames, latência adicionada, CPU do relay, drops por backpressure
e reconexão. Usar frames `Data`/`Deliver` sintéticos e tráfego WireGuard real.

| Métrica | Alvo preliminar | Medido | Status |
| --- | --- | --- | --- |
| Frames/s (payload 1 KB) | medir | a medir | a medir |
| Throughput relay (Mbps) | ≥ 50 | a medir | a medir |
| Latência adicionada p95 | medir | a medir | a medir |
| Drops por backpressure | 0 em regime | a medir | a medir |
| CPU do relay a 100 Mbps | medir | a medir | a medir |
| Reconexão após queda | < 30 s | a medir | a medir |

## 11. Teste funcional (smoke)

Não é benchmark de performance, mas precisa passar antes de publicar números: um IP de overlay
não garante que o serviço em loopback esteja acessível.

| Teste | Ação | Esperado | Status |
| --- | --- | --- | --- |
| Baguette direto | operador em `10.66.0.2` abre `http://10.66.0.3:8421` | HTTP 200 e UI carrega | a medir |
| Baguette via relay | N6 (UDP bloqueado), mesma URL | HTTP 200 e UI carrega | a medir |
| Porta não exposta | tentar `10.66.0.3:<porta fora da allowlist>` | conexão recusada | a medir |
| Host header | abrir com Host de overlay | Baguette aceita ou forward reescreve | a medir |
| ACL | nó sem tag de operador tenta a porta 8421 | drop, sem resposta | a medir |

## 12. Resumo para o dono

Preencher só depois das medições:

| Pergunta | Resposta | Evidência |
| --- | --- | --- |
| O direto funciona na maioria dos Macs? | a medir | matriz §8 |
| O relay aguenta os casos ruins? | a medir | §10 |
| O throughput serve para build/print? | a medir | §4 |
| 250 nós cabem em um VPS? | a medir | §7 |
| A memória do agente é aceitável? | a medir | §6 |
| A ACL não mata a latência? | a medir | §9 |

## 13. Artefatos

Guardar em `benchmarks/<AAAA-MM-DD>/<cenário>/`:

- `config.toml`/`env` (sem segredos);
- `raw.csv` (amostras);
- `summary.md` (p50/p95/p99, CPU, memória);
- comandos usados (`run.sh`);
- versão do agente/controle e commit.

Não anexar chaves privadas, tokens ou PCAPs com tráfego sensível. PCAP de teste deve ser
sintético ou anonimizado.


## Smoke Docker (medido no ambiente de desenvolvimento)

Estes números **não são benchmark de produção**: três containers Linux na mesma bridge do
Docker Desktop (Apple Silicon), control/relay no mesmo host, sem NAT real e sem `netem`.
Servem para detectar regressões grosseiras e comparar direct vs relay.

| Cenário | Comando | Resultado observado |
|---|---|---|
| probe direto site→a1:8421 (via `--expose` loopback) | `wm probe a1 8421` | 2–4 ms |
| ping ICMP direto site→a1 | `wm ping a1` | 2–3 ms |
| probe via relay site→a2 (force-relay) | `wm probe a2 8421` | 5–13 ms |
| probe negado pela ACL (a2→a1) | `wm probe a1 8421` | timeout ~5 s |
| ping com control parado | `wm ping a1` | continua (RTT ~7–17 ms no teste) |
| RTT reportado pelo mapa | `wm peers --json` | direct 1–4 ms; relay 5–13 ms |

Ambiente: Docker 29.9.2, imagem Debian bookworm, kernel linuxkit 7.0.14 aarch64, rede bridge
172.22.0.0/16. O relay e o control dividem o mesmo processo/container. Para números de
produção, siga a seção 1 (três execuções, 60 s, `tc netem`, hardware real, VPS com RTT real).

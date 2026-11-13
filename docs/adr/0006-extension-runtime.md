# ADR 0006 — Runtime de extensões sobre mapa + report, com sidecars no agente

- **Status:** Aceito; implementação inicial
- **Data:** 2026-11-12
- **Decisores:** dono + implementação
- **Relacionado:** [extensions.md](../extensions.md),
  [ADR 0001](0001-data-plane-boringtun.md), [ADR 0002](0002-control-plane-map.md),
  [ADR 0003](0003-relay-transport.md)

## Contexto

O wired-magic já entrega malha, ACL, relay e painel. Operar projetos dentro da
malha ainda exige ações manuais: subir um proxy, publicar um serviço, abrir uma
porta para fora. Queremos um mecanismo em que novas funcionalidades entrem como
**extensões** — pelo menos uma para expor portas de um nó pela Cloudflare
(cloudflared) e um caminho declarativo para sidecars futuros.

Restrições:

- o control plane é um binário Rust/axum + SQLite, sem HA, sem Redis;
- o agente é o único componente privilegiado e cria TUN;
- a operação é uma pessoa; cada peça nova aumenta a superfície;
- o estado já converge por `NetworkMap`/long-poll e `ReportRequest`/30 s;
- o painel é React + Vite, servido pelo próprio control.

## Decisão

1. **Reutilizar o mapa e o report como transporte de extensões.**
   `NetworkMap.extensions: Vec<ExtensionDesired>` é a lista autoritativa por nó;
   `ReportRequest.extensions` devolve o observado. Não há API de dados, socket
   ou porta nova.
2. **Separar control side e node side.**
   O control side declara o manifesto, guarda configuração e produz o desejo; o
   node side é um supervisor de sidecars. O acoplamento é o JSON de config, não
   uma chamada síncrona.
3. **Manifesto em disco, código opcional.**
   `<data_dir>/extensions/<id>/extension.toml` descreve a extensão. O loader
   valida e ignora manifestos quebrados com aviso. Extensões declarativas usam
   um driver genérico (`exec`); extensões com lógica registram um
   `ControlExtension` em Rust. O manifesto do cloudflared é semeado no primeiro
   boot e tem fallback embutido.
4. **Drivers explícitos e allowlisted.**
   `NodeDriver` é `none`, `exec` ou `cloudflared`. O agente só executa drivers
   conhecidos; `NodeDriver` vem do control, não do JSON arbitrário.
5. **Revisão monotônica por (extensão, nó).**
   O agente reconcilia por `revision + enabled + config`; mudanças de config e o
   botão “reiniciar” incrementam a revisão. Isso evita reiniciar sidecars quando
   o mapa muda por peers/ACL e permite um restart explícito.
6. **Lista autoritativa.**
   Extensão ausente do mapa é parada; extensão desabilitada recebe `enabled:
   false`; extensão removida do manifesto desaparece. Em `401`/`pending` o
   agente limpa a lista e para sidecars (revogação); em erro de rede mantém o
   último desejo (tolerância à queda do control).
7. **Status observado, não logs.**
   Cada extensão reporta `state`, `message`, `public_urls` e `details` JSON. O
   control guarda a última foto em `extension_status`, remove status de
   extensões sumidas e o painel faz merge com a configuração. Logs completos
   ficam no nó (journald/Console).
8. **Cloudflared como primeira extensão.**
   Modos `quick` (uma URL efêmera por rota, sem conta) e `named` (um processo
   por nó, ingress renderizado de credenciais/hostnames). O alvo é sempre
   loopback do próprio nó; credenciais só vão no mapa do dono e nunca voltam na
   API. O agente grava credentials `0600`.
9. **UI no painel React.**
   A lista de extensões e o gerenciador de rotas cloudflared são páginas do
   painel existente; extensões declarativas usam um editor JSON validado pelo
   driver. A API admin é a mesma para humanos e automação.

## Alternativas

| Alternativa | Prós | Contras | Veredito |
| --- | --- | --- | --- |
| **Mapa + report + sidecar** | zero transporte novo, auth/versionamento prontos, offline tolerante | acopla extensões ao payload do mapa; status não é tempo real | **escolhido** |
| API HTTP dedicada por nó | separação, payload sob demanda | nova auth, long-poll próprio, mais código e mais superfície | rejeitado no v0 |
| WebSocket de controle bidirecional | push imediato | estado de conexão, reconexão, auth de upgrade; o relay já usa WSS | rejeitado |
| Plugin dinâmico (`.so`/WASM) | carregar código sem recompilar | ABI/sandbox, segurança, toolchain, difícil depurar | futuro; não no v0 |
| Extensão como container/Kubernetes | isolamento | exige runtime externo, não é o modelo do projeto | rejeitado |
| Serviço externo de extensões | desacoplamento | mais um deploy, banco e auth; contradiz “um binário” | rejeitado |
| Shell script arbitrário no control | simples | sem validação, sem UI, sem report, perigoso | rejeitado |
| Cloudflared embutido no control | zero agent change | o control não está no nó, não alcança o loopback do projeto; errado por desenho | rejeitado |
| Quick tunnel apenas | zero configuração | URL efêmera e pública, sem hostname estável | insuficiente; suportamos named |

## Consequências

- **Payload.** `NetworkMap` cresce com extensões e shares. O mapa já é O(n); o
  custo adicional é pequeno para dezenas de rotas. Medir se virar problema.
- **Convergência.** Mudanças usam o mesmo wake-up do long-poll; em control fora,
  sidecars continuam. O status pode ficar defasado até o control voltar.
- **Privilégio.** O driver `exec` executa como root; a mitigação é o modelo de
  admin confiável, validação de config e manifesto em diretório root-owned. Não
  há sandbox no v0.
- **Segredos.** Credenciais ficam no SQLite (0600) e viajam no mapa TLS do nó
  dono. Não há KMS; backup do banco é sensível. A API mascara o segredo.
- **Cloudflared.** O binário precisa estar instalado no nó; sem ele o status é
  `error` com instrução. Named exige DNS/credenciais que o wired-magic não cria.
  Quick é público e não deve ser usado para produção.
- **Painel.** Ganha duas páginas e um editor genérico. Extensões declarativas
  não ganham UI sob medida; é JSON validado.
- **Testes.** O contrato é testável sem cloudflared real: fake scripts cobrem
  spawn, restart, parsing de URL e reconciliação; a API é testada via axum
  `oneshot`.

## Critérios de aceite

- `GET /api/v1/admin/extensions` lista a extensão embutida e manifestos
  declarativos válidos; manifesto quebrado não impede o boot.
- Criar rota quick/named no painel aparece no mapa do nó dono em < 1 s (long-poll
  acordado); status sai no report seguinte.
- Config idêntica não reinicia sidecar quando o mapa muda por outro motivo.
- Remover/desabilitar extensão para o processo; `401`/`pending` também param.
- Alvo não-loopback é recusado; credenciais nunca aparecem na API de leitura.
- `cloudflared` ausente vira `error` legível, não crash do agente.
- Testes de `wm-ext`, API e supervisor passam em macOS e Linux.

# Implementação — acompanhamento

Objetivo ativo: implementar o plano de prontidão até uma versão testável. Não equivale a aceite em produção.

## Rodada 1 — 2026-11-13

- Corrigido fail-open da compilação de ACL: default agora depende da política original vazia, não do resultado da expansão.
- Adicionadas regressões para seletores de nome/tag inexistentes e política intencionalmente vazia.
- Verificação: `CARGO_HOME="$PWD/.cargo-home" cargo test --offline -p wm-control acl::tests` — 4 passaram, exit 0.
- Política intencionalmente vazia mantém compatibilidade alpha allow-all; perfil restrito de produção ainda pendente.
- Em implementação paralela: revogação efetiva do relay/invalidação de fluxos; proteção da GUI/correção SPA. Ainda sem conclusão integrada nesta anotação.

### Continuação — autoridade de tags

- Enrollment ignora tags livres enviadas pelo cliente; autoridade vem das tags do convite criado pelo administrador.
- Bootstrap do host agora emite convite com as tags do próprio host, preservando `gateway` sem abrir autoatribuição para clientes.
- Regressão HTTP envia `site` com convite autorizado apenas para `mac` e verifica que o mapa contém somente `mac`.
- Verificação: `CARGO_HOME="$PWD/.cargo-home" cargo test --offline -p wm-control --test api` — 2 passaram, exit 0.
- Labels reservadas, identidade por nome e atomicidade do convite ainda pendentes.

### Integração das correções

- GUI loopback sem token no HTML público, bootstrap autenticado com capacidade em fragmento, proteção Host/Origin e regressões HTTP locais implementadas.
- SPA com MIME HTML correto em deep links e 404 para assets ausentes implementada.
- Relay com cancelamento explícito e validação de geração de sessão; conntrack revalida autorização após mapa/ACL novos.
- Enrollment, saída e autorrenomeação agora disparam recompilação de ACL. Regressão adiciona terceiro nó após política e verifica expansão de 4 para 9 regras.
- Integração API após recompilação: 2 testes aprovados, exit 0.
- E2E começou a receber assertions JSON para probes positivos e negação por timeout; convites separados de site/mac e retirada do token dos logs. Ainda falta isolamento, assert de caminhos e execução Docker real.
- Suíte integrada: `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline` — **71 aprovados, 1 ignorado**, exit 0. Inclui regressões GUI HTTP, SPA, relay, conntrack e autoridade de tags. `bash -n scripts/e2e-docker.sh` passou antes da execução; Docker/TUN/NAT reais ainda não executados.

## Rodada 2

- Enrollment agora consome o convite e cria nó/token na mesma transação SQLite. O primeiro UPDATE condicional adquire lock de escrita; convite expirado/revogado/esgotado falha sem criar nó. Falha de criação faz rollback do uso.
- Labels de convite prevalecem sobre labels informadas pelo cliente; tags continuam exclusivamente administrativas.
- Novas regressões DB: tentativa com chave duplicada não gasta convite; retry funciona; duas inscrições concorrentes com convite single-use têm exatamente um vencedor; sem tokens/nós órfãos.
- Verificação integrada: `pnpm --dir web typecheck`, `pnpm --dir web build`, `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline`: todos exit 0; **78 testes aprovados, 1 ignorado**.
- Geração systemd/launchd sem shell, quoting e XML seguros, opções persistidas explícitas e IDs separados implementados com 5 regressões. Guia: [Serviços de teste](SERVICOS-TESTE.md). Revisão pediu preservar host `--no-agent` não-root; ajuste em andamento.
- E2E isolado por UUID/labels, cleanup com ownership, asserts nome/caminho direct/relay e JSON probe, timeout e HTTP marcador por execução implementados. Contratos offline: `bash -n scripts/e2e-docker.sh`, `python3 scripts/test_e2e_assert.py` e versão `python3 -O` passaram (6 testes em cada execução). Sabotagens reais opt-in ainda não executadas. Guia: [E2E assertivo](E2E-ASSERTIVO.md). Não foi executado Docker privilegiado.

### Continuação — nomes ACL e revisão de serviços

- Enrollment que escolhe nome já referenciado em ACL passa a pending, exigindo aprovação administrativa. Autorrenomeação não pode assumir nem liberar nome referenciado; rename administrativo permanece possível.
- Verificação de reserva e rename serializada com escrita SQLite, evitando check/update separados. Erro de leitura/parse da política falha fechado.
- Nova regressão comprova pending, bloqueio de rename para/de identidade protegida e rename amigável comum funcionando. API/enrollment/shares: 7 testes passaram, exit 0.
- Revisão serviços concluída: host `--no-agent` e join `--no-start` usam apenas lock data-dir, preservando execução não-root. Corrigido join já inscrito que ignorava `--no-start`. Suíte CLI reportada pelo agente: 10/10, exit 0.
- Documentação de segurança atualizada com confiança em tags/nomes, rollback de convites, revalidação de fluxos e limites de revogação durante partições.

## Rodada 3 — candidato para testes locais

- GUI mantém comando explícito de início sudo com data-dir absoluto/quoting mesmo após enrollment/refresh; diferencia inscrito de conectado, polling sem sobreposição e estado degradado sem listas/status saudáveis antigos. Suíte GUI: 8 testes; cobertura contratual/HTTP, não DOM real.
- IP de login/enrollment/auditoria agora vem do socket TCP em HTTP e TLS; cabeçalhos forwarded e cabeçalho interno falsificados não mudam bucket. Regressão API passou. Proxy compartilha bucket nesta versão (não existe trusted proxy configurável).
- Limiter limita memória a 20.000 buckets, tamanho de chave e falha fechado em mutex/saturação, sem evict/reset de origens ativas; teste de fronteira/expiração incluído.
- ACL YAML/JSON persistida corrompida retorna erro, não converte em política default-allow; regressão também verifica rollback de uso de convite nesse erro.
- Novo smoke HTTP usa binário real, processo/porta/dados privados isolados; cobre login/CSRF, enrollment/aprovação/tags, uso único, rollback por UNIQUE WireGuard, mapa, ACL/simulação, SPA/assets e logout. Não usa TUN/Docker/privilégios nem revela credenciais.
- `bash scripts/verify-local.sh` encadeia Web→Rust→build→smoke e contratos E2E, guardando logs e manifesto SHA-256 em diretório único `evidence/local/`. Verificação final completa aprovada: **87 testes Rust passaram, 1 preview manual ignorado**; typecheck/build Web, sintaxe E2E e 6 contratos E2E (normal e `-O`), build e smoke HTTP real passaram. Smoke confirmou 0 fontes/assets posteriores ao binário. Logs: [Rust](../evidence/local/run.cSYN5gqs/rust-tests.log), [smoke](../evidence/local/run.cSYN5gqs/local-smoke.log), [manifesto](../evidence/local/run.cSYN5gqs/manifest.json). Manifesto identifica o snapshot de código/artefatos testados; esta atualização documental de resultados é posterior.

## Rodada 4 — TUI operacional e conexão persistente

- TUI passou a ser um console do membro, não só um visualizador: onboarding com código `wmc_...`
  ou URL+token, início/parada/reinício do agente, auto-start, ajustes de UDP/MTU/relay/TUN/expose,
  publicação e edição de shares, GUI local, serviço systemd/launchd, doctor, extensões e jobs.
- Agente iniciado pela TUI roda destacado (`setsid`, stdin fechado, stdout/stderr em `agent.log`);
  `q`/Ctrl+C encerram só a interface. O processo continua ativo, reconecta ao reabrir e é parado
  explicitamente por `X`, `:down` ou `wm down`. O log é rotacionado em 8 MiB.
- Console `:` executa qualquer subcomando `wm` sem shell e sem `--data-dir` alternativo; comandos
  longos (`up`, `join`, `host`, `serve`, `gui`) são destacados e monitorados. Comandos rápidos
  transmitem stdout/stderr e são encerrados se a TUI fechar.
- Estado da TUI (`tui-state.json`, 0600) guarda aba, seleção, histórico, configurações, auto-start,
  jobs e o último retrato de peers/shares/extensões; eventos em `tui-events.jsonl` (rotacionado).
  Convites, tokens e argumentos sensíveis são redigidos antes da gravação. Ao abrir com o agente
  parado, a TUI mostra o último retrato identificado como “última sessão”.
- Testes novos cobrem parsing/roteamento de comandos, redação de segredos, round-trip do estado,
  formas e navegação, deduplicação dos eventos de parada, renderização de todas as abas, o
  fetcher real contra um socket falso, jobs persistentes/log e o ciclo de vida de um agente
  destacado falso (socket Unix, sessão própria, SIGTERM). Suíte integrada:
  `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline` — **124 aprovados, 1 preview
  manual ignorado**, exit 0.
- Não executado nesta rodada: TUI em TTY real (o ambiente de agente não permite abrir pty), agente
  real com TUN/root, jobs `host`/`serve` reais, instalação systemd/launchd e navegador da GUI.
  Permanecem dependentes dos testes de campo já listados abaixo.

## Rodada 5 — controle por mouse

- `wm tui` ativa `EnableMouseCapture` (e desativa na saída) com mapa de hit regions gerado a cada
  frame: abas clicáveis, botões de ação contextuais por aba, linhas de tabela com clique/duplo
  clique, roda para listas/console/ajuda e clique direito para o menu completo de ações.
- Menu de ações (botão **Menu**, tecla `m` ou clique direito) cobre o caminho por mouse para
  todas as operações: agente, shares, peers, jobs, serviço, host/serve locais, GUI, painel admin,
  abas, console e saída. Formulários e confirmações têm campos clicáveis e botões
  Confirmar/Cancelar.
- Duplo clique ativa a linha apenas quando ação e coordenada coincidem, evitando abrir o item
  errado quando a tabela rola para mostrar a seleção. Tabelas usam o offset real do Ratatui para
  mapear linhas visíveis; overlays são hit-testados antes da UI de trás.
- `--no-mouse` desliga a captura para seleção nativa de texto; `Shift+arrastar` também é o
  escape usual em terminais que suportam. Hover usa eventos de movimento quando o terminal os
  envia, com fallback visual nos botões.
- Testes novos cobrem troca de aba, menu/console via clique, clique em linha e duplo clique,
  mapeamento de linhas com offset/rolagem, ciclo/foco/cancelamento de formulário (inclusive em
  80 colunas), confirmação de remoção, confirmação de host, roda, clique direito, menu até “Sair”,
  integridade e não sobreposição das hit regions, além de renderização em 1×1/20×8/40×12/60×18. Suíte integrada:
  `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline` — **137 aprovados, 1 preview
  manual ignorado**, exit 0.
- Não executado nesta rodada: TUI num TTY real com mouse (o ambiente não abre pty), terminal
  físico/multiplexador e seleção de texto no terminal. O comportamento é coberto por
  `TestBackend` e eventos de mouse sintéticos; confirme com o terminal real.

## Rodada 6 — identidade, várias redes e exigências por nó

- Identidade de exibição separada da identidade ACL: `display_name`/`about` no enrollment,
  editáveis pelo próprio nó (`PATCH /api/v1/node/profile` e `set_profile` na API local),
  propagadas no mapa para painéis, TUI e CLI. O nome técnico continua sendo a identidade usada
  por ACL e não muda ao personalizar a apresentação.
- TUI multi-rede: `profiles.json` (0600) na raiz do data-dir, `default` como perfil legado
  (data-dir antigo), demais perfis em `profiles/<slug>/`. Aba **Redes** alterna a rede ativa,
  cria/renomeia/remove perfis e inicia/para agentes independentes; `wm tui --profile <slug>`
  abre ou cria o perfil. Agentes de outros perfis continuam rodando após a troca; colisão de
  overlay entre perfis ativos aparece como `⚠`.
- Exigências por nó no control plane: versão mínima, tags e extensões obrigatórias, e
  parâmetros do agente (UDP, MTU, relay, expose, TUN). O agente reporta `version` e o runtime
  observado; `compliance::evaluate` compara e publica `NodeCompliance` no `NodeView`/mapa.
  Não conformidade é sinalizada, nunca desconecta o nó. Editor no painel (**Nós → editar**),
  leitura na TUI e `PUT /api/v1/admin/nodes/{id}/requirements`.
- Aprovação por convite: `require_approval` opcional no convite (padrão `false`, só convite).
  Quando ligado, os nós daquele convite entram `pending` até aprovação administrativa.
- Regressões novas: 3 testes HTTP de identidade/exigências/aprovação, 5 testes de perfis na
  TUI e 3 testes de conformidade; suíte integrada abaixo.
- Verificação desta rodada: `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline`
  — **211 aprovados, 2 ignorados** (inclui as regressões de desktop/PTY integradas em paralelo); `pnpm --dir web typecheck` e `pnpm --dir web build` exit 0.
- Smoke HTTP no binário real (`wm host --no-agent`, banco novo): enrollment com `display_name`
  normalizado, mapa expondo a identidade, `PATCH /node/profile` sem mudar o nome técnico,
  exigências gravadas, relatório observado reprovando (versão/MTU/extensão) e depois aprovando,
  e convite com `require_approval` entrando `pending` — todos PASS. Estado descartável removido.
- Não executado: TUI em TTY real, troca de perfil com dois agentes TUN reais, painel web em
  navegador e validação de conformidade sob TUN/Docker. Multi-tenant (várias malhas com ACL e
  tokens isolados no mesmo processo de control) **não** entrou: cada rede é um control plane
  próprio e a TUI gerencia os vários perfis/servidores.

## Marco e próximos passos externos

**Implementado para testes locais, não aprovado para produção.** Reproduzir com [teste local](TESTE-LOCAL.md). Serviços: [roteiro](SERVICOS-TESTE.md). Rede: [E2E assertivo](E2E-ASSERTIVO.md).

Permanecem para aceite completo do plano:

1. Docker/TUN em runner autorizado, executar sabotagens reais route/relay/policy e enforcement TCP/UDP.
2. Browser real (GUI e SPA), incluindo login/refresh/degradação/reconexão; GUI possui provas HTTP/contratuais, SPA possui smoke HTTP mas não DOM.
3. Instalação/reboot/upgrade/rollback em Linux/macOS limpos; locks cobrem caminhos CLI, não promessa universal de exclusão de agentes iniciados fora deles.
4. Definir política inicial de produção restrita: alpha ainda preserva política intencionalmente vazia allow-all. Proxy/limites de conexão, revisão independente, fmt/clippy e auditoria de dependências pendentes (`rustfmt` indisponível localmente).
5. Confirmar VPS/Macs/jogos/portas/plataformas e realizar campo/NAT/performance. Integração Seance exige outro repositório; API disponível não é integração implementada. Nenhum gate G0–G5 está implicitamente aprovado.

## Rodada 7 — release v0.1.6

- Consolidação das duas linhas paralelas: a árvore local (identidade e perfis, exigências por nó,
  bridge desktop macOS) e a linha publicada v0.1.4/v0.1.5 (TUIs de extensão em panes PTY).
- Merge de `origin/main` (v0.1.5) resolvido em `README.md` (mantida a versão nova), `wm-proto`
  (comentário) e `wm-tui/src/lib.rs` (linha em branco).
- Versão `0.1.6` em `Cargo.toml`, `web/package.json` e manifests flake; docs de Homebrew e
  referência técnica atualizados.
- Verificação: `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline` — **214 aprovados,
  2 ignorados**; `pnpm --dir web typecheck` e `pnpm --dir web build` exit 0.
- Publicação: assets macOS `arm64` e `x86_64`, `SHA256SUMS`, `wired-magic.rb` e fórmula no tap.

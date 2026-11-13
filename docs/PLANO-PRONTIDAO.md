# Plano de prontidão — wired-magic

Data: 2026-11-13. Estado: **candidato implementado para testes locais; aceite de campo/release pendente**.

Implementação e evidências parciais: [status](IMPLEMENTACAO-STATUS.md). Para reproduzir a
verificação sem privilégios, veja [teste local](TESTE-LOCAL.md) e execute
`bash scripts/verify-local.sh`. Este marco não fecha G0–G5: os checkboxes abaixo continuam
sendo critérios de aceite completo, não apenas presença de código/testes unitários.

## 1. Objetivo e origem do escopo

Transformar o alpha atual em uma entrega instalável, segura e comprovada nos cenários abaixo. Este plano recupera requisitos dos documentos do projeto, não das conversas privadas com outros agentes. Itens não registrados precisam de confirmação do dono; não são requisitos presumidos.

Fontes: [runbook Tauá](RUNBOOK-TAUA.md), [integração Seance](INTEGRATION-SEANCE.md), [shares](SHARES.md), [deploy](../deploy/README.md), [segurança](SECURITY.md) e [benchmarks](benchmarks.md).

### Cenários de aceite

| ID | Cenário | Resultado obrigatório |
|---|---|---|
| C1 | Seance na VPS → Macs do rack atrás de NAT/firewall | Aluno usa somente o site HTTPS do Seance; servidor alcança Baguette em loopback via overlay/expose; autenticação pedagógica e builds continuam sob responsabilidade do Seance. Sem port forwarding no laboratório. |
| C2 | Desenvolvedor compartilha app local ou Docker com outro membro | Entrar por convite/código, publicar porta, descobrir por nome e abrir via GUI/CLI. Validar serviço ligado ao overlay e serviço em loopback. Nenhuma porta pública adicional no nó. |
| C3 | Membros compartilham servidor de jogo TCP/UDP | Endereço copiável e conexão real entre redes distintas; testar o jogo e as portas escolhidos pelo dono. Não prometer descoberta LAN/broadcast ou suporte universal a jogos. |
| C4 | UDP bloqueado/NAT desfavorável e indisponibilidade | Relay preserva acesso quando direto não funciona; troca de rede e reconexão são verificadas. Queda do control não interrompe tráfego direto já estabelecido. |
| C5 | Operação diária sem intervenção de desenvolvedor | Instalar, reiniciar máquina, restaurar serviço, diagnosticar falha, atualizar e voltar à versão anterior sem perder identidade/configuração. |
| C6 | URL pública via Cloudflare, se requerida | Publicar somente rota explicitamente autorizada; validar quick/named, retirada da publicação e proteção de credenciais. **Condicional: não confundir com share privado.** |

### Confirmar antes do aceite em campo

- [ ] Quantos Macs/membros simultâneos e quais versões/arquiteturas de Linux/macOS?
- [ ] Qual VPS, domínio, redes reais e janela autorizada de testes?
- [ ] Quais apps, jogos, portas e transportes são obrigatórios? Windows é necessário para os jogadores?
- [ ] C6 entra nesta entrega? Quick, named ou ambos?
- [ ] Qual repositório/responsável implementará a integração do Seance?
- [ ] Aprovar limites mensuráveis de latência, throughput, recuperação e duração do piloto.

Windows/mobile, exit node, subnet routing, HA, SSO e DNS do sistema não entram automaticamente. Se um cenário exigir algum deles, reabrir o escopo antes de declarar prontidão. Contagem de 250 nós nos benchmarks é hipótese de medição, não requisito confirmado.

## 2. Linha de base e regra de conclusão

Na avaliação de 2026-11-13, `CARGO_HOME="$PWD/.cargo-home" cargo test --workspace --offline` passou com 56 testes aprovados e 1 ignorado; `pnpm --dir web typecheck` passou. Isso é uma observação da árvore de trabalho, ainda sem commit de referência; não é evidência de uma release reproduzível. `cargo fmt` não estava instalado no ambiente usado. Não foram reexecutados browser, TUN, Docker, NAT real ou deploy.

Revisão estática encontrou bloqueadores de autorização, revogação, GUI e confiabilidade dos testes. Os registros de smoke Docker existentes não provam funcionamento no laboratório. A integração externa do Seance está documentada como futura.

**Concluído** exige: código revisado + teste de regressão + execução na plataforma pertinente + evidência associada ao commit + documentação coerente. Não aceitar apenas “compila”, captura de tela ou mensagem de conclusão de agente. Todos os itens deste plano começam pendentes; não representam trabalho já atribuído.

## 3. Ordem de execução e responsáveis propostos

Responsáveis abaixo são papéis a atribuir, não agentes já despachados.

| Etapa | Responsável proposto | Depende de | Saída/gate |
|---|---|---|---|
| M0 — baseline e contratos | coordenação + QA | — | G0: escopo, decisões e commit-base registrados |
| M1 — segurança do núcleo | backend/control + core | M0 | G1: autorização, revogação e regressões aprovadas |
| M2 — instalação e interfaces | CLI/deploy + frontend | M0; aceite após G1 | G2: onboarding limpo e operação segura Linux/macOS |
| M3 — provas automatizadas | QA + rede | M0; fechamento após M1/M2 | G3: suíte assertiva e artefatos reproduzíveis |
| M4 — integração e campo | integração Seance + operador | G1, G2, G3 | G4: cenários reais aprovados |
| M5 — release e operação | release + operador + dono | G4 | G5: release, restauração e aceite final |

M1, M2 e infraestrutura de M3 podem avançar em paralelo, com contratos congelados em M0. O mesmo dono deve coordenar alterações concorrentes em enrollment, CLI e GUI. C6 não deve atrasar o núcleo se for retirado explicitamente do escopo.

## 4. Backlog implementável

### M0 — Fixar o que será entregue

- [ ] **P00 — Baseline:** revisar arquivos não versionados, excluir segredos/artefatos locais, criar commit-base e registrar toolchain/lockfiles. Saída: identificação reproduzível da árvore, sem credenciais no Git.
- [ ] **P01 — Contratos:** decidir tags autorizativas, identidade estável para ACL, política de conexões após mudança de regra, comportamento de share desativado e segurança versus disponibilidade quando control está fora. Registrar decisões antes de codificar.
- [ ] **P02 — Matriz:** preencher plataformas, topologia, escala e apps/jogos obrigatórios. Mapear cada requisito a C1–C6 e a um teste. Sem acesso ao cenário real, manter aceite de campo pendente, não substituí-lo por containers.

### M1 — Bloqueadores de segurança (P0)

| ID | Implementação | Critério de aceite e regressão |
|---|---|---|
| S01 | Corrigir fail-open em [acl.rs](../crates/wm-control/src/acl.rs): distinguir política intencionalmente vazia de expansão sem correspondência. Perfil de produção deve começar restrito. | Política não vazia com nome/tag inexistente nega; remoção do último nó/tag não abre a malha. Testar combinação de seletores válidos/inválidos, regras vazias e simulação. |
| S02 | Em [node.rs](../crates/wm-control/src/api/node.rs), impedir autoatribuição de tags privilegiadas e sobrescrita de metadados reservados. Tags autorizativas vêm de admin/convite autorizado. Não usar nome livremente mutável como identidade de segurança. | Convite comum não obtém acesso de `site`, mesmo enviando tags/labels ou escolhendo/alterando nome. Testar aprovação manual e ativação automática. Atualizar CLI e runbooks para não depender de `--tag` autoautorizativo. |
| S03 | Cancelar explicitamente reader/writer do relay ao revogar nó ou substituir sessão; revalidar autorização no ciclo de conexão. | Socket antigo encerra, não encaminha novos frames e não interfere na nova sessão. Token revogado não reconecta. Testar corrida entre revogação e envio; peers atualizados rejeitam chave revogada. |
| S04 | Invalidar/reavaliar conntrack após alteração restritiva de ACL em [agent.rs](../crates/wm-core/src/agent.rs). | Fluxo TCP/UDP já ativo perde autorização após aplicação do mapa restritivo, mesmo sob tráfego contínuo. Respostas de fluxos ainda permitidos continuam válidas; cobrir ICMP. |
| S05 | Recompilar ACL em todas as alterações relevantes de identidade/tag/status, incluindo enrollment e rename; publicar versão consistente. | Novo nó autorizado recebe regras sem edição manual extra; nome/tag antigos não conservam privilégios. Testar rename, aprovação, revogação e reinício. |
| S06 | Tornar consumo de convite e criação do nó atômicos; definir retry idempotente sem permitir reutilização indevida. | Nome inválido/duplicado, labels inválidas, falha de criação ou pool cheio não queimam convite sem nó criado; concorrência respeita limite de usos. |
| S07 | Corrigir autenticação da [GUI](../crates/wm/src/gui.rs): HTML público não contém segredo; exigir capacidade válida para bootstrap/API; negar bind não-loopback por padrão. Revisar Origin/Host, CSRF, cache e logs. | GET anônimo não obtém token; API anônima/origem não autorizada é negada; credenciais não aparecem em logs. Testar bootstrap legítimo e configuração de bind externo. Não alegar proteção cross-origin sem teste. |
| S08 | Endurecer IP de origem/rate limit: confiar forwarded headers somente de proxy explicitamente confiável; limitar memória e conexões. | Cliente direto não contorna limiter trocando headers; proxy real identifica origem corretamente; nenhuma credencial aparece em erros. |

**Gate G1:** testes negativos dos itens S01–S08 aprovados; revisão independente das fronteiras de autorização; nenhum bloqueador de segurança conhecido pendente. Explicitar que revogação global instantânea não é garantida enquanto peers estão isolados do control. Se necessária, implementar leases/expiração e testar o custo de disponibilidade; não prometer simultaneamente operação offline ilimitada e revogação imediata.

### M2 — Jornada completa, interfaces e serviços

- [ ] **U01 — Onboarding:** instalar em máquina limpa, entrar com convite/código, conceder privilégio necessário e iniciar serviço persistente. GUI diferencia “inscrito” de “agente conectado”; não esconder instrução indispensável após refresh. Escolher e documentar fluxo seguro de elevação, sem executar shell arbitrário vindo do browser.
- [ ] **U02 — Serviço:** gerar systemd/launchd com argumentos e escaping corretos, preferindo execução direta sem `sh -c`; testar caminhos com espaços, aspas e caracteres XML. Persistir URL, data-dir, `--expose` e opções necessárias após reboot. Impedir dois agentes/rotas concorrentes; separar modo host/agent.
- [ ] **U03 — SPA:** corrigir MIME do fallback em [spa.rs](../crates/wm-control/src/api/spa.rs); HTML deve ser `text/html`, assets inexistentes devem retornar erro apropriado. Testar login, link direto e refresh de todas as rotas.
- [ ] **U04 — Estado verdadeiro:** falha de polling/API mostra estado degradado e última atualização; não representar erro como lista vazia ou manter “conectado” indefinidamente. Cobrir recuperação sem recarregar página.
- [ ] **U05 — Shares:** testar criação, conflito, rename, toggle e exclusão pelo dono; mutação por outro membro deve falhar. Definir se desligar apenas remove descoberta ou também bloqueia tráfego no IP:porta. Se prometer bloqueio, implementar enforcement e testar acesso direto e fluxos existentes; ausência no catálogo não é firewall.
- [ ] **U06 — GUI/TUI/CLI:** comandos e mensagens consistentes para peer offline, ACL negada, serviço ausente, token expirado e URL inválida. `wm://` continua resolução interna salvo implementação e teste explícitos de protocolo do sistema.
- [ ] **U07 — Pacotes:** preparar binários por SO/arquitetura acordados, integridade/autenticidade verificável, seleção correta no instalador e erro seguro para plataforma não suportada. Não depender do binário do host ser compatível com o cliente.

**Gate G2:** operador que não desenvolveu o recurso executa C2 e instala/reinicia host e agente em Linux/macOS limpos usando somente a documentação. Nenhuma inscrição é confundida com conexão ativa; instalação, upgrade e rollback preservam identidade e expose.

### M3 — Testes que realmente reprovam falhas

- [ ] **T01 — E2E assertivo:** em [e2e-docker.sh](../scripts/e2e-docker.sh), retirar tolerância a falhas dos passos obrigatórios, usar timeout explícito e assertions de JSON/caminho/resposta. Teste negativo deve exigir negação, não apenas ignorar o exit code. Garantir teardown e evidências mesmo em falha; usar recursos isolados para não afetar deploy existente.
- [ ] **T02 — Sabotagem do teste:** quebrar propositalmente rota, relay e política em ambiente descartável; cada quebra precisa reprovar o caso correspondente. “E2E concluído” só aparece após todos os asserts.
- [ ] **T03 — Browser:** automatizar login/logout/CSRF, refresh/deep links, nós/tokens/ACL, GUI sem token, shares e indicação de erro/reconexão. Testar backend real; mock não vale para aceite de integração.
- [ ] **T04 — CI:** Rust tests/fmt/clippy, TypeScript/build, browser e regressões de segurança; E2E privilegiado em runner isolado. Auditar dependências e definir política de severidade/exceção. Falta de runner não deve virar teste silenciosamente ignorado.
- [ ] **T05 — Release consistente:** build do frontend precede embed/build Rust; falha web interrompe release. Fixar ferramentas, registrar hashes e provar que binário contém o painel da mesma revisão, não um dist antigo.
- [ ] **T06 — Provas de rede:** separar LAN/Docker de redes com NAT, CGNAT ou UDP bloqueado. Medir direto, relay, reconexão e mudança de rede; evitar inferir caminho pelo sucesso de um ping.

**Gate G3:** pipeline reproduzível, falhas induzidas detectadas, nenhum `|| true` mascarando critérios obrigatórios e resultados associados ao commit candidato.

### M4 — Integração externa e validação em campo

- [ ] **I01 — Seance:** implementar no repositório correto o cliente da API read-only, cache/timeout, merge por identidade acordada e estado degradado. Token só server-side. Não declarar integração pronta apenas porque API do wired-magic responde.
- [ ] **I02 — Baguette real:** do container Seance alcançar `/simulators.json` e executar a jornada real de build/resultado prevista pelo sistema. Browser do aluno acessa somente Seance; proxy verifica autorização do usuário, não expõe token nem endpoint privado do Mac.
- [ ] **I03 — Rotas:** validar uma estratégia de rede do container (host ou bridge+SNAT), interface real e CIDR exato, persistência após reboot e ausência de conflito com WireGuard/mesh antigo. Documentar rollback das regras de firewall sem regras amplas desnecessárias.
- [ ] **I04 — Isolamento Tauá:** site alcança apenas portas aprovadas dos Macs; Mac→Mac e membro comum→Baguette são negados. SSH 22 só entra se explicitamente necessário. Testar resposta legítima, tentativa não autorizada e mudança de ACL durante sessão.
- [ ] **I05 — App/jogo real:** executar C2/C3 em redes distintas, com os apps/jogos definidos em P02, nos dois caminhos. Confirmar TCP/UDP, múltiplas portas, firewall local e endereço anunciado. Teste HTTP não substitui jogo UDP.
- [ ] **I06 — Cloudflare (condicional):** usuário autorizado publica e retira rota, credenciais não voltam pela API/log, sidecar é encerrado e configuração sobrevive a reboot. Revisar execução de extensões sob root e modelo de confiança. URL pública não herda automaticamente a ACL privada; exigir autenticação apropriada no serviço/proxy.

### Matriz mínima de falhas de campo

| Caso | Verificação |
|---|---|
| LAN/direto | Caminho autenticado direto e tráfego real nos dois sentidos permitidos. |
| Redes distintas/NAT | Resultado observado e endpoints registrados; não exigir direto em NAT que não o permita. |
| UDP bloqueado | Relay confirmado por telemetria e tráfego real; DNS/HTTPS disponíveis. |
| Direto → relay → direto | Perda/latência/interrupção medidas; confrontar promessa de continuidade com evidência. |
| Control parado | Direto estabelecido continua; painel indica degradação. Se control e relay compartilham processo, derrubá-lo também derruba relay: não exigir sobrevivência relay-only nesse caso. |
| Relay parado | Direto continua; relay-only falha de modo explícito e se recupera após retorno. |
| Reboot/sleep/wake/troca de rede | Agente retorna sem novo convite, recupera rotas/expose e não duplica instância. |
| Revogação/ACL restritiva | Bloqueio nos peers com mapa atualizado e fechamento do relay; registrar comportamento durante partição. |
| Backup/restore/upgrade | Identidade/config preservadas; cliente conecta e API/serviço respondem após restauração. |

**Gate G4:** C1–C5 aprovados em plataformas/redes reais; C6 aprovado ou formalmente fora do escopo. Proposta de piloto: 24 h de uso contínuo com falhas induzidas em janela acordada. Duração e limites de desempenho precisam ser aprovados; não são garantias atuais.

### M5 — Entrega operável

- [ ] **R01:** medir RTT, throughput, CPU/RSS, reconexão e carga no tamanho real acordado; definir orçamento de relay/VPS. Sem números inventados ou extrapolação de bridge Docker.
- [ ] **R02:** alertas/logs para perda do control/relay, nó offline, serviço parado e erros de autenticação; rotação/retenção, backup protegido e restauração ensaiada.
- [ ] **R03:** corrigir documentação desatualizada sobre conntrack, placeholders, tags livres, segurança e benchmarks; separar recurso implementado, validado localmente e aprovado em campo.
- [ ] **R04:** publicar release com hashes, versões suportadas, migrações, limitações e procedimentos de instalação/upgrade/rollback. Ensaio em máquinas limpas, não somente na máquina de desenvolvimento.
- [ ] **R05:** operador e dono assinam checklist por cenário e aceitam explicitamente riscos residuais. Nenhum problema P0/P1 de segurança ou fluxo obrigatório em aberto; exceções de baixo impacto devem ter responsável e prazo.

## 5. Evidências e acompanhamento

Criar, durante a implementação, um pacote `evidence/<release>/<cenario>/` contendo: commit, hash do binário/painel, SO/arquitetura/topologia, comandos, resultado esperado/observado, saída/exit code, dados brutos de medições e data. Sanitizar tokens, chaves, dados pessoais e tráfego sensível. A pasta `evidence/local/` já contém provas locais com logs e manifestos SHA-256; elas não substituem provas de campo/release.

Cada tarefa deve registrar responsável, estado (`pendente`, `em execução`, `em revisão`, `aprovado` ou `bloqueado`), PR/commit, teste e link de evidência. O implementador não é o único aprovador de itens de segurança. Se bloqueada por acesso à VPS, hardware ou repositório externo, registrar a dependência; não marcar concluída por simulação.

## 6. Definição final de pronto

- [ ] G0–G5 aprovados com evidências verificáveis.
- [ ] Cenários e plataformas exigidos pelo dono explicitamente cobertos.
- [ ] Sem escalada por tags/nome, fail-open acidental ou segredo público na GUI.
- [ ] Revogação, ACL e partições têm comportamento implementado e documentado.
- [ ] Instalação limpa, reboot, atualização e rollback comprovados.
- [ ] Seance integrado e jornada real do laboratório aprovada, não apenas contrato HTTP.
- [ ] Apps/jogos reais funcionam nas redes alvo, inclusive relay quando necessário.
- [ ] Operador consegue diagnosticar/restaurar sem chamar o autor do código.

**Próxima ação:** atribuir M0 e S01–S08; em paralelo preparar U01–U03 e T01–T03. Não abrir novas funcionalidades fora do escopo até fechar os bloqueadores. Não há estimativa de calendário confiável sem responsáveis, plataformas, acesso aos ambientes e requisitos de campo confirmados.
